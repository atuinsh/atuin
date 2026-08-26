//! Structure which performs repeated periodic operations.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;

/// A background task that runs `work` on a fixed period, aborting the task when dropped.
///
/// The task lives exactly as long as the [`PeriodicTask`] handle: hold it for as long as the
/// work should keep running, and drop it (or let it fall out of scope) to stop.
pub struct PeriodicTask {
    task: JoinHandle<()>,
}

impl PeriodicTask {
    /// Run `work` immediately, then re-run it once every `period`.
    pub fn spawn_now<F, Fut>(period: Duration, work: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::spawn(period, true, work)
    }

    /// Wait one `period`, then run `work`, and re-run it once every `period` after that.
    pub fn spawn_later<F, Fut>(period: Duration, work: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::spawn(period, false, work)
    }

    fn spawn<F, Fut>(period: Duration, eager: bool, mut work: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);

            // The first tick of an interval always completes immediately. Consume it up front
            // when the caller asked to wait a full period before the first run.
            if !eager {
                ticker.tick().await;
            }

            loop {
                ticker.tick().await;
                work().await;
            }
        });

        Self { task }
    }
}

impl Drop for PeriodicTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for PeriodicTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeriodicTask").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;

    use super::*;

    fn counter() -> (Arc<AtomicUsize>, impl FnMut() -> std::future::Ready<()> + Send + 'static) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let work = move || {
            seen.fetch_add(1, Ordering::SeqCst);
            std::future::ready(())
        };
        (calls, work)
    }

    #[rstest]
    #[tokio::test]
    async fn spawn_now_runs_immediately() {
        let (calls, work) = counter();
        // A period far longer than the test window: only the immediate run should land.
        let _task = PeriodicTask::spawn_now(Duration::from_secs(3600), work);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn spawn_later_waits_one_period() {
        let (calls, work) = counter();
        let _task = PeriodicTask::spawn_later(Duration::from_millis(150), work);

        // Before the first period elapses, nothing has run.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // After a full period, it has run at least once.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[rstest]
    #[tokio::test]
    async fn repeats_each_period() {
        let (calls, work) = counter();
        let _task = PeriodicTask::spawn_now(Duration::from_millis(30), work);

        tokio::time::sleep(Duration::from_millis(200)).await;
        // Immediate run plus several more; assert conservatively to avoid timing flakiness.
        assert!(calls.load(Ordering::SeqCst) >= 3);
    }

    #[rstest]
    #[tokio::test]
    async fn drop_aborts_the_task() {
        let (calls, work) = counter();
        let task = PeriodicTask::spawn_now(Duration::from_millis(20), work);

        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(task);
        let after_drop = calls.load(Ordering::SeqCst);

        // Long enough for several more ticks had it not been aborted.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let later = calls.load(Ordering::SeqCst);

        // At most one in-flight run may complete after the abort; no further ticks fire.
        assert!(later <= after_drop + 1, "{later} grew past {after_drop} + 1");
    }
}
