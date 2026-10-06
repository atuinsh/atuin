//! A cap on how much blocking work runs at once on tokio's blocking pool.

use std::num::NonZeroUsize;
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Runs blocking closures on tokio's blocking pool, at most `max_workers` at a time.
///
/// A closure keeps its worker until it returns, even when its caller stops waiting: the work is
/// still running, so it still counts. Clones share one cap.
#[derive(Debug, Clone)]
pub struct BlockingPool {
    workers: Arc<Semaphore>,
}

/// Errors returned by [`BlockingPool::run`].
#[derive(Debug, thiserror::Error)]
#[error("blocking work was cancelled by its runtime shutting down")]
pub struct BlockingCancelled;

impl BlockingPool {
    /// A pool running at most `max_workers` closures at once, capped at
    /// [`Semaphore::MAX_PERMITS`].
    #[must_use]
    pub fn new(max_workers: NonZeroUsize) -> Self {
        Self {
            workers: Arc::new(Semaphore::new(max_workers.get().min(Semaphore::MAX_PERMITS))),
        }
    }

    /// Run `f` on tokio's blocking pool once a worker is free, resuming any panic in `f` here.
    ///
    /// # Errors
    ///
    /// [`BlockingCancelled`] when the runtime shuts down before `f` finishes.
    pub async fn run<T, F>(&self, f: F) -> Result<T, BlockingCancelled>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let worker = Arc::clone(&self.workers)
            .acquire_owned()
            .await
            .expect("the pool never closes its semaphore");
        let work = tokio::task::spawn_blocking(move || {
            let _worker = worker;
            f()
        });
        match work.await {
            Ok(value) => Ok(value),
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(_) => Err(BlockingCancelled),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use rstest::rstest;

    use super::*;

    fn pool(max_workers: usize) -> BlockingPool {
        BlockingPool::new(NonZeroUsize::new(max_workers).unwrap())
    }

    #[rstest]
    #[tokio::test]
    async fn run_returns_the_closures_value() {
        assert_eq!(pool(1).run(|| 7).await.unwrap(), 7);
    }

    #[rstest]
    #[tokio::test]
    async fn work_past_the_cap_does_not_start_until_a_worker_frees() {
        let pool = pool(2);
        let (release, parked) = mpsc::channel::<()>();
        let parked = Arc::new(parking_lot::Mutex::new(parked));
        let running: Vec<_> = (0..2)
            .map(|_| {
                let (pool, parked) = (pool.clone(), Arc::clone(&parked));
                tokio::spawn(async move { pool.run(move || parked.lock().recv()).await })
            })
            .collect();
        while pool.workers.available_permits() > 0 {
            tokio::task::yield_now().await;
        }

        let started = Arc::new(AtomicBool::new(false));
        let late = tokio::spawn({
            let (pool, started) = (pool.clone(), Arc::clone(&started));
            async move { pool.run(move || started.store(true, Ordering::SeqCst)).await }
        });
        tokio::task::yield_now().await;
        // Both workers are held until `release` sends, so the late closure cannot have run yet.
        let started_early = started.load(Ordering::SeqCst);

        // Unpark before asserting: a panic with work still parked would hang runtime shutdown.
        release.send(()).unwrap();
        release.send(()).unwrap();
        for work in running {
            work.await.unwrap().unwrap().unwrap();
        }
        late.await.unwrap().unwrap();
        assert!(!started_early, "work past the cap started before a worker freed");
        assert!(started.load(Ordering::SeqCst));
    }

    /// The worker must follow the blocking closure, not the awaiting caller: a caller that gives
    /// up leaves the closure running, and that running closure is what the cap is for.
    #[rstest]
    #[tokio::test]
    async fn a_cancelled_caller_keeps_its_worker_until_the_closure_returns() {
        let pool = pool(1);
        let (release, parked) = mpsc::channel::<()>();
        let run = pool.run(move || parked.recv());
        assert!(tokio::time::timeout(Duration::from_millis(50), run).await.is_err());
        assert_eq!(pool.workers.available_permits(), 0, "worker freed while the closure ran");

        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), pool.run(|| ()))
            .await
            .expect("the worker frees once the closure returns")
            .unwrap();
    }

    #[rstest]
    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn a_panic_in_the_closure_resumes_in_the_caller() {
        let _ = pool(1).run(|| panic!("boom")).await;
    }

    #[rstest]
    #[tokio::test]
    async fn clones_share_one_cap() {
        let pool = pool(1);
        let clone = pool.clone();
        let (release, parked) = mpsc::channel::<()>();
        let run = tokio::spawn(async move { clone.run(move || parked.recv()).await });
        while pool.workers.available_permits() > 0 {
            tokio::task::yield_now().await;
        }
        assert!(tokio::time::timeout(Duration::from_millis(50), pool.run(|| ())).await.is_err());

        release.send(()).unwrap();
        run.await.unwrap().unwrap().unwrap();
    }
}
