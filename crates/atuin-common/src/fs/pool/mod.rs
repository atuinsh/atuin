//! An [`FdPool`] lends out file descriptors up to its limit of leases, and callers past it wait for
//! one to come back.
//!
//! Pools form a tree: [`FdPool::child`] carves out a sub-pool with its own [`Quota`], and a lease
//! counts against the pool it came from and every ancestor. [`FdPool::system`] is the process-wide
//! root, sized to the descriptors the process can still open.
//!
//! Note that a [`Lease`] is a slot which can hold a file descriptor, not a descriptor itself.
//!
//! Tie it to the descriptor it pays for with [`Lease::hold`], or run a whole open-use-close
//! sequence under one with [`FdPool::blocking`], so the slot cannot come back while the descriptor
//! is still open.
//!
//! [`FdPool`]s can also contain children [`FdPool`]s, enabling you to have a tree of pools:
//!
//! ```text
//! pool-a (max 100)
//!   -> pool-a-1 (max 50)
//!   -> pool-a-2 (max 45)
//!
//! ```
//!
//! In this example, `pool-a` can [`Lease`] five FDs at its own level.

mod system;

use std::fmt::Debug;
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};

use crate::sync::Wakeup;

/// How long a waiter sleeps before re-checking, for limits that move without a returned lease.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The most leases a pool may hold at once: a fixed number, or one that follows something outside
/// the pool.
trait LeaseLimit: Send + Sync + Debug {
    /// The most leases the pool may hold, given it holds `held` now.
    fn max_leases(&self, held: usize) -> usize;

    /// Hear that work under a lease on this pool or a descendant ran out of descriptors.
    fn on_exhausted(&self) {}
}

impl LeaseLimit for NonZeroUsize {
    fn max_leases(&self, _held: usize) -> usize {
        self.get()
    }
}

/// How many leases a child pool may hold, and how many of those its parent guarantees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Leases held at once by the pool and its descendants.
    pub limit: NonZeroUsize,
    /// Of those, how many the parent keeps for this pool alone, whatever its siblings hold.
    pub reserve: usize,
}

/// Errors returned by [`FdPool::child`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuotaError {
    #[error("a reserve of {reserve} is over the limit of {limit}")]
    ReserveOverLimit {
        reserve: usize,
        limit: usize,
    },
    #[error("the parent would reserve {reserved} of its {limit} slots")]
    ParentOverReserved {
        reserved: usize,
        limit: usize,
    },
}

/// A pool of file-descriptor leases; see the module docs.
#[derive(Debug)]
pub struct FdPool {
    position: TreePosition,
    limit: Box<dyn LeaseLimit>,
    // TODO(markovejnovic): We could avoid this [`Mutex`], but it makes the code really messy and I
    //                      decided it's not worth it.
    counts: Mutex<Counts>,
}

/// Where a pool sits in its tree.
#[derive(Debug)]
enum TreePosition {
    /// The top of the tree.
    Root {
        returned: Wakeup,
    },
    /// Under `parent`, which keeps `reserve` of its slots for this pool alone.
    Child {
        parent: Arc<FdPool>,
        reserve: usize,
    },
}

#[derive(Debug, Default)]
struct Counts {
    /// Leases held by this pool and its descendants.
    held: usize,
    /// Leases counted against this pool's shared slots: its own, and its children's past their
    /// reserves.
    shared: usize,
    /// The sum of this pool's children's reserves.
    reserved: usize,
}

impl FdPool {
    /// A root pool that lends out at most `limit` leases.
    #[must_use]
    fn new(limit: impl LeaseLimit + 'static) -> Arc<Self> {
        Arc::new(Self {
            position: TreePosition::Root {
                returned: Wakeup::new(),
            },
            limit: Box::new(limit),
            counts: Mutex::default(),
        })
    }

    /// Carve a sub-pool out of this one.
    pub fn child(self: &Arc<Self>, quota: Quota) -> Result<Arc<Self>, QuotaError> {
        let (limit, reserve) = (quota.limit.get(), quota.reserve);
        if reserve > limit {
            return Err(QuotaError::ReserveOverLimit { reserve, limit });
        }

        let mut counts = self.counts.lock();
        let reserved = counts.reserved.saturating_add(reserve);
        let limit = self.limit.max_leases(counts.held);
        if reserved > limit {
            return Err(QuotaError::ParentOverReserved { reserved, limit });
        }
        counts.reserved = reserved;
        drop(counts);

        Ok(Arc::new(Self {
            position: TreePosition::Child {
                parent: Arc::clone(self),
                reserve,
            },
            limit: Box::new(quota.limit),
            counts: Mutex::default(),
        }))
    }

    /// Wait for a lease.
    pub async fn acquire(self: &Arc<Self>) -> Lease {
        // Polled too: a limit can grow without any lease returning.
        //
        // The reason the limit can grow is because some unmanaged file descriptor can be retained,
        // meaning that we no longer hit EMFILE.
        self.returned()
            .until_or_every(POLL_INTERVAL, || {
                self.try_take().then(|| Lease {
                    pool: Arc::clone(self),
                })
            })
            .await
    }

    /// Run `f` on the blocking pool under a lease, for descriptors `f` opens and closes itself.
    pub async fn blocking<T, F>(self: &Arc<Self>, f: F) -> io::Result<T>
    where
        F: FnOnce() -> io::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let lease = self.acquire().await;
        let result = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            f()
        })
        .await
        .expect("given closure panicked");

        // Only Unix reports running out of descriptors, as `EMFILE`.
        #[cfg(unix)]
        let exhausted = matches!(
            &result,
            Err(err) if err.raw_os_error() == Some(rustix::io::Errno::MFILE.raw_os_error())
        );
        #[cfg(not(unix))]
        let exhausted = false;
        if exhausted {
            self.path().for_each(|pool| pool.limit.on_exhausted());
        }

        result
    }

    /// Leases held by this pool and its descendants.
    pub fn held(&self) -> usize {
        self.counts.lock().held
    }

    /// This pool and its ancestors, from this pool up to the root.
    fn path(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |pool| match &pool.position {
            TreePosition::Root { .. } => None,
            TreePosition::Child { parent, .. } => Some(parent),
        })
    }

    /// The root's wakeup, shared by the whole tree.
    fn returned(&self) -> &Wakeup {
        match &self.position {
            TreePosition::Root { returned } => returned,
            TreePosition::Child { parent, .. } => parent.returned(),
        }
    }

    /// Slots the parent keeps for this pool alone; the root has no parent to keep any.
    const fn reserve(&self) -> usize {
        match &self.position {
            TreePosition::Root { .. } => 0,
            TreePosition::Child { reserve, .. } => *reserve,
        }
    }

    /// Lock this pool's counts and every ancestor's.
    ///
    /// Always from the pool up to the root, the one order every caller takes, so none deadlock.
    fn lock_path(&self) -> Vec<(&Self, MutexGuard<'_, Counts>)> {
        self.path().map(|pool| (pool, pool.counts.lock())).collect()
    }

    /// Take a lease here if this pool and every ancestor admit one, and say whether it did.
    fn try_take(&self) -> bool {
        let mut path = self.lock_path();
        let limits: Vec<usize> =
            path.iter().map(|(pool, counts)| pool.limit.max_leases(counts.held)).collect();
        let admits = {
            let has_shared_slot =
                |i: usize| path[i].1.shared < limits[i].saturating_sub(path[i].1.reserved);
            path[0].1.held < limits[0]
                && has_shared_slot(0)
                && (1..path.len()).all(|i| {
                    let (child, counts) = &path[i - 1];
                    path[i].1.held < limits[i]
                        && (counts.held < child.reserve() || has_shared_slot(i))
                })
        };
        if !admits {
            return false;
        }
        path[0].1.held += 1;
        path[0].1.shared += 1;
        for i in 1..path.len() {
            let (child, counts) = &path[i - 1];
            let past_reserve = counts.held > child.reserve();
            let parent = &mut path[i].1;
            parent.held += 1;
            parent.shared += usize::from(past_reserve);
        }
        true
    }

    /// Return a lease taken here.
    fn release(&self) {
        let mut path = self.lock_path();
        path[0].1.held -= 1;
        path[0].1.shared -= 1;
        for i in 1..path.len() {
            // Leases are interchangeable, so the one returned is past the reserve iff any is.
            let (child, counts) = &path[i - 1];
            let was_past_reserve = counts.held >= child.reserve();
            let parent = &mut path[i].1;
            parent.held -= 1;
            parent.shared -= usize::from(was_past_reserve);
        }
    }
}

impl Drop for FdPool {
    fn drop(&mut self) {
        // The last handle, lease and child of this pool are gone, and with them its reserve.
        if let TreePosition::Child { parent, reserve } = &self.position {
            parent.counts.lock().reserved -= reserve;
        }
    }
}

/// A slot in an [`FdPool`], returned on drop.
#[must_use = "a dropped lease returns its slot at once"]
#[derive(Debug)]
pub struct Lease {
    pool: Arc<FdPool>,
}

impl Lease {
    /// Tie this lease to the descriptor it pays for, so both are released together.
    #[must_use]
    pub fn hold<T>(self, fd: T) -> Leased<T> {
        Leased { fd, _lease: self }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.release();
        self.pool.returned().wake_all();
    }
}

/// A descriptor `T` together with the [`Lease`] that pays for it.
#[derive(Debug, derive_more::Deref, derive_more::DerefMut)]
pub struct Leased<T> {
    // Declared first so it drops first: the descriptor closes before its slot returns.
    #[deref]
    #[deref_mut]
    fd: T,
    _lease: Lease,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use rstest::rstest;

    use super::*;

    fn pool(limit: usize) -> Arc<FdPool> {
        FdPool::new(NonZeroUsize::new(limit).unwrap())
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_acquire_past_the_limit_waits_for_a_lease_to_return() {
        let pool = pool(1);
        let lease = pool.acquire().await;
        assert!(tokio::time::timeout(Duration::from_secs(1), pool.acquire()).await.is_err());

        let waiter = tokio::spawn({
            let pool = pool.clone();
            async move { drop(pool.acquire().await) }
        });
        tokio::task::yield_now().await;
        drop(lease);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("a returned lease wakes the waiter")
            .unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn a_held_descriptor_keeps_its_lease_until_it_drops() {
        let pool = pool(1);
        let dir = tempfile::tempdir().unwrap();
        let file = pool.acquire().await.hold(std::fs::File::create(dir.path().join("f")).unwrap());
        assert_eq!(pool.held(), 1);
        file.metadata().unwrap();
        drop(file);
        assert_eq!(pool.held(), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn children_draw_on_their_parent() {
        let pool = pool(2);
        let child = pool
            .child(Quota {
                limit: NonZeroUsize::new(2).unwrap(),
                reserve: 1,
            })
            .unwrap();
        let _a = child.acquire().await;
        let _b = pool.acquire().await;
        assert_eq!(pool.held(), 2);
        assert!(tokio::time::timeout(Duration::from_millis(50), child.acquire()).await.is_err());
    }

    /// The lease must follow the blocking work, not the awaiting caller: a caller that gives up
    /// leaves `f` running with its descriptors open, and those are what the limit is for.
    #[rstest]
    #[tokio::test]
    async fn a_cancelled_blocking_call_keeps_its_lease_until_the_call_ends() {
        let pool = pool(1);
        let (release, parked) = mpsc::channel::<()>();
        let call = pool.blocking(move || {
            let _ = parked.recv();
            Ok(())
        });
        assert!(tokio::time::timeout(Duration::from_millis(50), call).await.is_err());
        assert_eq!(pool.held(), 1, "the lease returned while the call still ran");

        release.send(()).unwrap();
        drop(
            tokio::time::timeout(Duration::from_secs(5), pool.acquire())
                .await
                .expect("the lease returns once the blocking call ends"),
        );
    }

    #[rstest]
    #[tokio::test]
    async fn a_blocking_call_past_the_limit_does_not_start() {
        let pool = pool(2);
        let (release, parked) = mpsc::channel::<()>();
        let parked = Arc::new(parking_lot::Mutex::new(parked));
        let running: Vec<_> = (0..2)
            .map(|_| {
                let (pool, parked) = (pool.clone(), Arc::clone(&parked));
                tokio::spawn(async move {
                    pool.blocking(move || {
                        let _ = parked.lock().recv();
                        Ok(())
                    })
                    .await
                })
            })
            .collect();
        while pool.held() < 2 {
            tokio::task::yield_now().await;
        }

        let started = Arc::new(AtomicBool::new(false));
        let late = tokio::spawn({
            let (pool, started) = (pool.clone(), Arc::clone(&started));
            async move {
                pool.blocking(move || {
                    started.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
            }
        });
        tokio::task::yield_now().await;
        // Both leases are held and neither can return until `release` sends, so this is not a race.
        let started_early = started.load(Ordering::SeqCst);

        // Unpark before asserting: a panic with a call still parked would hang runtime shutdown.
        release.send(()).unwrap();
        release.send(()).unwrap();
        for call in running {
            call.await.unwrap().unwrap();
        }
        late.await.unwrap().unwrap();
        assert!(!started_early, "a call past the limit started before a lease returned");
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(pool.held(), 0);
    }
}
