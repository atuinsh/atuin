//! The process-wide pool, sized to the descriptors the process can still open.

use std::io;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::time::Instant;

use super::{FdPool, LeaseLimit};
use crate::os::fd::{self, FdLimit};

/// How stale the count of outside descriptors may get before the limit recounts it.
const RECOUNT_INTERVAL: Duration = Duration::from_millis(100);

/// Descriptors the system pool leaves to code outside the pools.
///
/// Unfortunately, we cannot account for all the file descriptors that a process may use, so we
/// leave some floating headroom to assume that there are file descriptors not managed by this pool.
const HEADROOM: usize = 64;

impl FdPool {
    /// The process-wide root pool, sized to the descriptors the process can still open.
    #[must_use]
    pub fn system() -> &'static Arc<Self> {
        static SYSTEM: LazyLock<Arc<FdPool>> = LazyLock::new(|| FdPool::new(SystemLimit::new()));
        &SYSTEM
    }
}

/// The process's descriptor limit, less [`HEADROOM`], less the descriptors open outside the pool.
#[derive(Debug)]
struct SystemLimit {
    /// What `counted_at` is measured from.
    epoch: Instant,
    /// When the process's descriptors were last counted, in milliseconds after `epoch` plus one;
    /// `0` forces a recount.
    counted_at: AtomicU64,
    /// A [`LeaseCapacity::pack`]ed [`LeaseCapacity`].
    capacity: AtomicU64,
}

/// How many leases the system pool may hand out, and how that number was arrived at.
#[derive(Debug, Clone, Copy)]
enum LeaseCapacity {
    /// Not counted yet: a failed first count falls back to headroom alone.
    Unknown,
    /// From the last count that succeeded, or was unsupported.
    Counted(usize),
    /// Counts are failing: kept from before they began, which was already warned of.
    Failing(usize),
}

impl LeaseCapacity {
    /// Bits of a packed capacity below its variant tag.
    const LEASE_BITS: u32 = 62;
    const LEASE_MASK: u64 = (1 << Self::LEASE_BITS) - 1;

    /// This capacity as one `u64`: the variant in the top two bits, the leases below, capped at
    /// [`Self::LEASE_MASK`], which is as good as unbounded.
    fn pack(self) -> u64 {
        let (tag, leases) = match self {
            Self::Unknown => (0, 0),
            Self::Counted(leases) => (1, leases),
            Self::Failing(leases) => (2, leases),
        };
        (tag << Self::LEASE_BITS) | u64::try_from(leases).unwrap_or(u64::MAX).min(Self::LEASE_MASK)
    }

    fn unpack(packed: u64) -> Self {
        let leases = usize::try_from(packed & Self::LEASE_MASK).unwrap_or(usize::MAX);
        match packed >> Self::LEASE_BITS {
            0 => Self::Unknown,
            1 => Self::Counted(leases),
            2 => Self::Failing(leases),
            tag => unreachable!("pack writes tags 0 to 2, not {tag}"),
        }
    }

    /// Leases the system pool may hold: `limit` less [`HEADROOM`], less `outside` descriptors
    /// held outside the pools.
    ///
    /// Never below one: a pool that admits nothing parks every caller forever, while one lease
    /// under a tiny limit at worst fails with `EMFILE`.
    fn size(limit: FdLimit, outside: usize) -> NonZeroUsize {
        let FdLimit::Bounded(limit) = limit else {
            return NonZeroUsize::MAX;
        };
        NonZeroUsize::new(limit.saturating_sub(HEADROOM).saturating_sub(outside))
            .unwrap_or(NonZeroUsize::MIN)
    }

    const fn leases(self) -> usize {
        match self {
            Self::Unknown => 0,
            Self::Counted(leases) | Self::Failing(leases) => leases,
        }
    }
}

impl SystemLimit {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            counted_at: AtomicU64::new(0),
            capacity: AtomicU64::new(LeaseCapacity::Unknown.pack()),
        }
    }

    /// Claim the recount if the last count is stale, so one caller counts and the rest carry on
    /// with the last capacity.
    fn claim_recount(&self) -> bool {
        let elapsed = Instant::now().duration_since(self.epoch).as_millis();
        let now = u64::try_from(elapsed).unwrap_or(u64::MAX).saturating_add(1);
        let last = self.counted_at.load(Ordering::Relaxed);
        if last != 0 && Duration::from_millis(now - last) < RECOUNT_INTERVAL {
            return false;
        }
        self.counted_at.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// Recount the process's descriptors into `capacity`.
    ///
    /// Only the caller that claimed the recount gets here. A recount still running when the next
    /// is claimed (a count slower than `RECOUNT_INTERVAL`, or one forced by `on_exhausted`) races
    /// it: the later store wins, and a failure may be warned of twice.
    fn recount(&self, held: usize) {
        let previous = LeaseCapacity::unpack(self.capacity.load(Ordering::Relaxed));
        // Counted with the pool locked, so acquirers wait out one count at most every
        // `RECOUNT_INTERVAL`. Leases taken but not yet opened make `open - held` undercount the
        // outside descriptors by at most the leases in flight, which the headroom absorbs.
        let next = match (fd::count_open(), previous) {
            (Ok(open), _) => LeaseCapacity::Counted(
                LeaseCapacity::size(fd::limit(), open.saturating_sub(held)).get(),
            ),
            (Err(err), _) if err.kind() == io::ErrorKind::Unsupported => {
                LeaseCapacity::Counted(LeaseCapacity::size(fd::limit(), 0).get())
            }
            (Err(_), failing @ LeaseCapacity::Failing(_)) => failing,
            (Err(err), LeaseCapacity::Counted(leases)) => {
                tracing::warn!(%err, "could not count open descriptors; keeping the pool at its last capacity");
                LeaseCapacity::Failing(leases)
            }
            (Err(err), LeaseCapacity::Unknown) => {
                tracing::warn!(%err, "could not count open descriptors; starting the pool sized by headroom alone");
                LeaseCapacity::Failing(LeaseCapacity::size(fd::limit(), 0).get())
            }
        };
        self.capacity.store(next.pack(), Ordering::Relaxed);
    }
}

impl LeaseLimit for SystemLimit {
    fn max_leases(&self, held: usize) -> usize {
        if self.claim_recount() {
            self.recount(held);
        }
        LeaseCapacity::unpack(self.capacity.load(Ordering::Relaxed)).leases()
    }

    fn on_exhausted(&self) {
        self.counted_at.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::outside_comes_off_the_top(FdLimit::Bounded(100), 20, 16)]
    #[case::zero_outside_leaves_the_headroom(FdLimit::Bounded(100), 0, 36)]
    #[case::overcommitted_still_admits_one(FdLimit::Bounded(100), 500, 1)]
    #[case::tiny_limit_still_admits_one(FdLimit::Bounded(32), 10, 1)]
    #[case::unbounded(FdLimit::Unbounded, 30, usize::MAX)]
    fn system_capacity_is_the_limit_less_headroom_and_outside(
        #[case] limit: FdLimit,
        #[case] outside: usize,
        #[case] expected: usize,
    ) {
        assert_eq!(LeaseCapacity::size(limit, outside).get(), expected);
    }
}
