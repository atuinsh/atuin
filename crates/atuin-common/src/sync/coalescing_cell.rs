//! A lazily-populated value whose refreshes are coalesced.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

/// A single lazily-populated value whose refreshes are coalesced: when several callers refresh at
/// once, the fetch runs exactly once and everyone who was waiting observes that single result.
///
/// This is the single-value case of Go's `singleflight`, except it also retains the value for
/// cheap reads via [`CoalescingCell::get`].
///
/// # Example
///
/// ```
/// # use atuin_common::sync::CoalescingCell;
/// # async fn run() {
/// let cell: CoalescingCell<u64> = CoalescingCell::default();
/// let value = cell.refresh(|| async { Ok::<_, std::convert::Infallible>(42) }).await.unwrap();
/// assert_eq!(*value, 42);
/// assert_eq!(cell.get().map(|v| *v), Some(42));
/// # }
/// ```
#[derive(Debug)]
pub struct CoalescingCell<T> {
    value: RwLock<Option<Arc<T>>>,
    generation: AtomicU64,
    refreshing: tokio::sync::Mutex<()>,
}

impl<T> CoalescingCell<T> {
    /// The current value, if it has been populated. Pure and non-blocking; clones an `Arc`.
    pub fn get(&self) -> Option<Arc<T>> {
        return self.value.read().clone();
    }

    /// Refresh the value via `fetch`, coalescing concurrent callers into a single run.
    ///
    /// The leader runs `fetch` and stores its result; every caller that was waiting returns that
    /// same value without running `fetch` again. A failed `fetch` does not count as a refresh, so
    /// the next waiter retries rather than coalescing onto the failure.
    pub async fn refresh<F, Fut, E>(&self, fetch: F) -> Result<Arc<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        // Snapshot the generation before we queue for the lock.
        let seen = self.generation.load(Ordering::Acquire);

        let _lead = self.refreshing.lock().await;

        // If a refresh landed while we waited, take its result instead of fetching again. The
        // generation only moves after a value is stored, so the value is guaranteed present.
        if self.generation.load(Ordering::Acquire) != seen {
            return Ok(self
                .value
                .read()
                .clone()
                .expect("generation moved, so a value was stored"));
        }

        // We are the leader: run the fetch with no data lock held, so reads stay concurrent.
        let fresh = Arc::new(fetch().await?);

        // Brief critical section, no `.await` inside -- never hold a parking_lot guard across one.
        *self.value.write() = Some(fresh.clone());
        self.generation.fetch_add(1, Ordering::Release);

        Ok(fresh)
    }
}

impl<T> Default for CoalescingCell<T> {
    fn default() -> Self {
        Self {
            value: RwLock::new(None),
            generation: AtomicU64::new(0),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Duration;

    use proptest::prelude::*;

    /// A stand-in fetcher that enforces the invariant a coalescing cell exists to provide: the
    /// fetch never runs concurrently with itself. `enter` returns a guard held across the fetch's
    /// `.await`; if a second fetch begins while one is in flight, `enter` panics -- so the assertion
    /// holds under whatever schedule the runtime happens to pick, not just a lucky one.
    struct FetchProbe {
        in_flight: AtomicBool,
        count: AtomicUsize,
    }

    impl FetchProbe {
        fn new() -> Self {
            Self {
                in_flight: AtomicBool::new(false),
                count: AtomicUsize::new(0),
            }
        }

        fn enter(&self) -> FetchGuard<'_> {
            assert!(
                !self.in_flight.swap(true, Ordering::SeqCst),
                "two fetches overlapped -- the cell failed to serialize them"
            );
            self.count.fetch_add(1, Ordering::SeqCst);
            FetchGuard(&self.in_flight)
        }

        fn count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    struct FetchGuard<'a>(&'a AtomicBool);

    impl Drop for FetchGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_never_overlaps_and_coalesces() {
        const CALLERS: usize = 100;

        let cell = Arc::new(CoalescingCell::<u64>::default());
        let probe = Arc::new(FetchProbe::new());
        // The barrier launches every caller together, so the herd genuinely contends.
        let barrier = Arc::new(tokio::sync::Barrier::new(CALLERS));

        let mut handles = Vec::with_capacity(CALLERS);
        for _ in 0..CALLERS {
            let cell = Arc::clone(&cell);
            let probe = Arc::clone(&probe);
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                cell.refresh(move || async move {
                    let _guard = probe.enter();
                    // Hold the fetch open long enough that the rest of the herd piles up behind it.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<u64, Infallible>(42)
                })
                .await
                .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(*handle.await.unwrap(), 42);
        }

        // Schedule-independent: the probe already proved no two fetches overlapped, and the herd
        // never did more work than the naive path.
        assert!((1..=CALLERS).contains(&probe.count()));
        // Expected under real contention: the whole herd collapses onto one fetch.
        assert_eq!(
            probe.count(),
            1,
            "the herd should collapse to a single fetch"
        );
        assert_eq!(cell.get().map(|v| *v), Some(42));
    }

    #[tokio::test]
    async fn get_is_empty_until_first_refresh() {
        let cell: CoalescingCell<u64> = CoalescingCell::default();
        assert!(cell.get().is_none());

        cell.refresh(|| async { Ok::<_, Infallible>(7) })
            .await
            .unwrap();

        assert_eq!(cell.get().map(|v| *v), Some(7));
    }

    #[tokio::test]
    async fn a_failed_fetch_stores_nothing_and_the_next_retries() {
        let cell: CoalescingCell<u64> = CoalescingCell::default();

        let err = cell
            .refresh(|| async { Err::<u64, &str>("boom") })
            .await
            .unwrap_err();
        assert_eq!(err, "boom");
        assert!(cell.get().is_none(), "a failed fetch stores nothing");

        // A later refresh still runs -- it did not coalesce onto the failure.
        let value = cell.refresh(|| async { Ok::<_, &str>(9) }).await.unwrap();
        assert_eq!(*value, 9);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Whatever the herd size and however the fetch is paced, the invariants hold: the fetch
        /// never overlaps itself (via the probe), the herd never does more work than the naive
        /// path, and every caller observes a consistent value.
        #[test]
        fn any_burst_stays_within_bounds(callers in 1usize..40, jitter_ms in 0u64..3) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            runtime.block_on(async move {
                let cell = Arc::new(CoalescingCell::<u64>::default());
                let probe = Arc::new(FetchProbe::new());
                let barrier = Arc::new(tokio::sync::Barrier::new(callers));

                let mut handles = Vec::with_capacity(callers);
                for _ in 0..callers {
                    let cell = Arc::clone(&cell);
                    let probe = Arc::clone(&probe);
                    let barrier = Arc::clone(&barrier);
                    handles.push(tokio::spawn(async move {
                        barrier.wait().await;
                        cell.refresh(move || async move {
                            let _guard = probe.enter();
                            if jitter_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                            } else {
                                tokio::task::yield_now().await;
                            }
                            Ok::<u64, Infallible>(42)
                        })
                        .await
                        .unwrap()
                    }));
                }

                for handle in handles {
                    prop_assert_eq!(*handle.await.unwrap(), 42);
                }
                prop_assert!((1..=callers).contains(&probe.count()));
                prop_assert_eq!(cell.get().map(|v| *v), Some(42));
                Ok(())
            })?;
        }
    }
}
