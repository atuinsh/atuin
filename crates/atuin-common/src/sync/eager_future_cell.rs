use std::future::Future;
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::{Notify, OnceCell};

#[derive(Debug)]
struct Inner<T> {
    /// The computed value, once it is ready.
    cell: OnceCell<T>,
    /// Notified once [`Self::cell`] is ready, waking anything waiting in [`EagerFutureCell::get`].
    ready: Notify,
}

/// A cell whose value is produced exactly once, eagerly, in the background.
///
/// [`EagerFutureCell::new`] accepts a future which will be executed in the background. Fetching the
/// value with [`EagerFutureCell::get`] will either wait for the future to produce the value, or
/// return the already-stored value as computed by the given future.
#[derive(Debug, Clone)]
pub struct EagerFutureCell<T> {
    inner: Arc<Inner<T>>,
}

impl<T: Send + Sync + 'static> EagerFutureCell<T> {
    /// Initialize the cell with the given `work` future, starting the work immediately if a tokio
    /// runtime is available.
    pub fn new<Fut>(work: Fut, handle: &Handle) -> Self
    where
        Fut: Future<Output = T> + Send + 'static,
    {
        let inner = Arc::new(Inner {
            cell: OnceCell::new(),
            ready: Notify::new(),
        });

        // Drive on a spawned task, so a `get` cancelled while waiting cannot strand the work.
        let driver = Arc::clone(&inner);
        handle.spawn(async move {
            let value = work.await;
            let _ = driver.cell.set(value);
            driver.ready.notify_waiters();
        });

        Self { inner }
    }

    /// Fetch the value, awaiting the work if necessary.
    pub async fn get(&self) -> &T {
        if let Some(value) = self.inner.cell.get() {
            return value;
        }

        loop {
            let notified = self.inner.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if let Some(value) = self.inner.cell.get() {
                return value;
            }

            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;

    use super::EagerFutureCell;

    #[rstest]
    #[case(42)]
    #[case(7)]
    #[tokio::test]
    async fn computes_once_and_caches(#[case] value: usize) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let cell = EagerFutureCell::new(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                value
            },
            &tokio::runtime::Handle::current(),
        );

        // Repeated gets return the cached value, and the work runs exactly once even though the
        // eager background kick and these gets can race.
        assert_eq!(*cell.get().await, value);
        assert_eq!(*cell.get().await, value);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn constructs_from_a_handle_outside_the_runtime() {
        // The explicit handle lets us construct (and eagerly spawn) from a thread that is not
        // itself running inside the runtime.
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cell = EagerFutureCell::new(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                7usize
            },
            rt.handle(),
        );

        assert_eq!(*rt.block_on(cell.get()), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
