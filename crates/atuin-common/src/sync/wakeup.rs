//! A wakeup for tasks waiting on a condition, free to signal when nobody waits.
//!
//! This utility is designed to use fewer cycles than plain [`tokio::sync::Notify`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

/// Wakes tasks waiting on a condition that other tasks change; see the module docs.
#[derive(Debug, Default)]
pub struct Wakeup {
    notify: Notify,
    /// Tasks inside [`Wakeup::until`] or [`Wakeup::until_or_every`] whose first check failed.
    waiting: AtomicUsize,
}

impl Wakeup {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            notify: Notify::const_new(),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Wake every task waiting, so each re-checks its condition.
    pub fn wake_all(&self) {
        // This feels really strange, but it is correct. The reason we do the fetch_add here is
        // because we want a fence.
        //
        // It's counter-intuitive but true. Let's assume this was a `load`. If it was a load, then
        // this could race with [`Registered::sync`].
        //
        // `load` firstly can't be acq-rel, it has to be `acq`. So imagine this:
        //
        // ```rust
        // if self.waiting.load(Ordering::Acquire) > 0 {
        //     self.notify.notify_waiters();
        // }
        // ```
        //
        // Imagine we've got two threads:
        // waiter                           waker
        // W1: waiting.fetch_add(1)         K1: cond = true
        // W2: enable()
        // W3: sync()   // fetch_add(0)     K2: waiting.load(Acquire)
        // W4: read cond                    K3: if > 0 { notify }
        // With a load at K2, this outcome is allowed:
        // - K2 reads waiting == 0: the waiter's +1 isn't visible to the waker yet, and a load is
        //   allowed to return a stale value.
        // - So K3 skips notify.
        // - W4 reads cond == false: the waker never wrote waiting, so W3 had nothing to acquire
        //   from, and K1's write isn't visible yet.
        //
        // But!!! The waker DID set cond = true, it's just that no release actually released it.
        //
        // Super counter-intuitive, but it is possible. The other option is to stick a seq-cst fence
        // but that's objectively worse.
        if self.waiting.fetch_add(0, Ordering::AcqRel) > 0 {
            self.notify.notify_waiters();
        }
    }

    /// Wait until `ready` returns `Some`, re-checking after every [`Wakeup::wake_all`].
    pub async fn until<T>(&self, ready: impl FnMut() -> Option<T>) -> T {
        self.wait(None, ready).await
    }

    /// Equivalent to [`Wakeup::until`], except it also re-checks every `interval`, for conditions
    /// that can change without anyone calling [`Wakeup::wake_all`].
    pub async fn until_or_every<T>(
        &self,
        interval: Duration,
        ready: impl FnMut() -> Option<T>,
    ) -> T {
        self.wait(Some(interval), ready).await
    }

    async fn wait<T>(&self, interval: Option<Duration>, mut ready: impl FnMut() -> Option<T>) -> T {
        if let Some(value) = ready() {
            return value;
        }

        let registered = Registered::new(&self.waiting);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Enabled before `sync`, so a `wake_all` that sees this waiter reaches it, unpolled.
            notified.as_mut().enable();
            registered.sync();
            if let Some(value) = ready() {
                return value;
            }
            match interval {
                None => notified.await,
                Some(interval) => {
                    let _ = tokio::time::timeout(interval, notified).await;
                }
            }
        }
    }
}

/// One task counted in [`Wakeup::waiting`], uncounted on drop, including when the wait is
/// cancelled.
struct Registered<'a>(&'a AtomicUsize);

impl<'a> Registered<'a> {
    fn new(waiting: &'a AtomicUsize) -> Self {
        // Relaxed: every check that follows is ordered by a `sync` after it.
        waiting.fetch_add(1, Ordering::Relaxed);
        Self(waiting)
    }

    /// Pair with [`Wakeup::wake_all`]'s read-modify-write before re-checking the condition.
    fn sync(&self) {
        self.0.fetch_add(0, Ordering::AcqRel);
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_waiter_wakes_once_its_condition_holds() {
        let wakeup = Arc::new(Wakeup::new());
        let flag = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn({
            let (wakeup, flag) = (Arc::clone(&wakeup), Arc::clone(&flag));
            async move {
                wakeup.until(|| flag.load(Ordering::Relaxed).then_some(())).await;
            }
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        flag.store(true, Ordering::Relaxed);
        wakeup.wake_all();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("wake_all wakes the waiter")
            .unwrap();
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn until_or_every_rechecks_without_a_wake() {
        let wakeup = Wakeup::new();
        let flag = AtomicBool::new(false);
        let wait = wakeup.until_or_every(Duration::from_millis(100), || {
            flag.load(Ordering::Relaxed).then_some(())
        });
        tokio::pin!(wait);
        assert!(tokio::time::timeout(Duration::from_millis(50), wait.as_mut()).await.is_err());

        flag.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_millis(200), wait)
            .await
            .expect("the next interval re-checks");
    }

    #[rstest]
    #[tokio::test]
    async fn a_cancelled_wait_stops_counting_as_waiting() {
        let wakeup = Wakeup::new();
        let wait = wakeup.until(|| None::<()>);
        assert!(tokio::time::timeout(Duration::from_millis(20), wait).await.is_err());
        assert_eq!(wakeup.waiting.load(Ordering::Relaxed), 0);
    }

    /// Many waiters and wakers on separate threads: a wakeup lost between a waiter's check and
    /// its sleep leaves that waiter hanging past the deadline.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_wakeup_is_lost_under_contention() {
        const ROUNDS: usize = 2_000;
        let wakeup = Arc::new(Wakeup::new());
        let turn = Arc::new(AtomicUsize::new(0));
        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let (wakeup, turn) = (Arc::clone(&wakeup), Arc::clone(&turn));
                tokio::spawn(async move {
                    for round in 1..=ROUNDS {
                        wakeup
                            .until(|| (turn.load(Ordering::Relaxed) >= round).then_some(()))
                            .await;
                    }
                })
            })
            .collect();
        for _ in 0..ROUNDS {
            turn.fetch_add(1, Ordering::Relaxed);
            wakeup.wake_all();
            tokio::task::yield_now().await;
        }
        for waiter in waiters {
            tokio::time::timeout(Duration::from_secs(10), waiter)
                .await
                .expect("every waiter saw every round")
                .unwrap();
        }
    }
}
