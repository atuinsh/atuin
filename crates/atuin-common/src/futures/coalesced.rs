use std::future::Future;
use std::sync::Arc;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// A background job that runs on request, one run at a time.
///
/// Requests coalesce: the requests that arrive while the job is running leave one more run
/// pending, however many of them arrive. Dropping the `Coalesced` aborts the job.
#[derive(Debug)]
pub struct Coalesced {
    requested: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Coalesced {
    /// Spawns the task that runs `job` on each [`request`](Self::request).
    ///
    /// # Panics
    ///
    /// Panics outside a Tokio runtime.
    pub fn spawn<F>(mut job: impl FnMut() -> F + Send + 'static) -> Self
    where
        F: Future<Output = ()> + Send,
    {
        let requested = Arc::new(Notify::new());
        let task = tokio::spawn({
            let requested = Arc::clone(&requested);
            async move {
                loop {
                    requested.notified().await;
                    job().await;
                }
            }
        });
        Self { requested, task }
    }

    /// Starts a run, or leaves one pending when a run is in progress.
    pub fn request(&self) {
        // `notify_one` stores at most one permit, which the task takes once the current run
        // ends.
        self.requested.notify_one();
    }
}

impl Drop for Coalesced {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;
    use tokio::sync::Semaphore;

    use super::*;

    #[rstest]
    #[tokio::test]
    async fn requests_during_a_run_leave_one_more_run() {
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));
        let job = Coalesced::spawn({
            let runs = Arc::clone(&runs);
            let gate = Arc::clone(&gate);
            move || {
                let runs = Arc::clone(&runs);
                let gate = Arc::clone(&gate);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    gate.acquire().await.expect("the gate is open").forget();
                }
            }
        });
        let settle = async || {
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            runs.load(Ordering::SeqCst)
        };
        assert_eq!(settle().await, 0);

        job.request();
        assert_eq!(settle().await, 1);
        for _ in 0..3 {
            job.request();
        }
        gate.add_permits(10);

        assert_eq!(settle().await, 2);
    }

    #[rstest]
    #[tokio::test]
    async fn dropping_aborts_the_job() {
        let runs = Arc::new(AtomicUsize::new(0));
        let job = Coalesced::spawn({
            let runs = Arc::clone(&runs);
            move || {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        let requested = Arc::clone(&job.requested);

        drop(job);
        requested.notify_one();
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }

        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }
}
