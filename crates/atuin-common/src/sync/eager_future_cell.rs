use std::future::Future;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::{Notify, OnceCell};
use tokio::task::AbortHandle;

/// A cell whose value is seeded with a task scheduled at [`EagerFutureCell::new`], in the
/// background.
///
/// [`EagerFuture::new`] accepts a future which is executed in the background. [`EagerFuture::get`]
/// either waits for the future to produce the value, or returns the already-stored value.
pub type EagerFutureCell<T> = EagerFuture<OnceCell<T>>;

/// A cell whose value is seeded with a task scheduled at [`MutEagerFutureCell::new`], in the
/// background. Unlike the [`EagerFutureCell`] one-shot dual, [`MutEagerFutureCell`] allows you to
/// emplace any arbitrary value into the cell, via the `overwrite` call.
pub type MutEagerFutureCell<T> = EagerFuture<Mutex<Option<T>>>;

impl<T: Clone + Send + Sync + 'static> MutEagerFutureCell<T> {
    /// Force `value` into the cell, aborting the background future so its (now-superseded) result is
    /// discarded. Any current or future [`get`](EagerFuture::get) observes `value`.
    pub fn overwrite(&self, value: T) {
        self.abort.abort();
        *self.inner.cell.lock() = Some(value);
        self.inner.ready.notify_waiters();
    }
}

/// Acts as a storage backend to [`EagerFutureCell`].
pub trait ResultCell: Default + Send + Sync + 'static {
    type Value: Clone + Send + Sync + 'static;

    /// Place the value into the cell.
    fn fill(&self, value: Self::Value);

    /// Read the value from the cell.
    fn peek(&self) -> Option<Self::Value>;
}

impl<T: Clone + Send + Sync + 'static> ResultCell for OnceCell<T> {
    type Value = T;

    fn fill(&self, value: T) {
        let _ = self.set(value);
    }

    fn peek(&self) -> Option<T> {
        self.get().cloned()
    }
}

impl<T: Clone + Send + Sync + 'static> ResultCell for Mutex<Option<T>> {
    type Value = T;

    fn fill(&self, value: T) {
        let mut slot = self.lock();
        // Keep an existing value: an `overwrite` may have already won.
        if slot.is_none() {
            *slot = Some(value);
        }
    }

    fn peek(&self) -> Option<T> {
        self.lock().clone()
    }
}

/// Data stored under the [`EagerFuture`].
#[derive(Debug)]
struct Inner<C> {
    cell: C,
    ready: Notify,
}

/// A cell whose value is seeded with a task scheduled at [`EagerFutureCell::new`], in the
/// background.
///
/// Use [`EagerFutureCell`] or [`MutEagerFutureCell`], directly.
pub struct EagerFuture<C> {
    inner: Arc<Inner<C>>,
    abort: AbortHandle,
}

impl<C> Clone for EagerFuture<C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            abort: self.abort.clone(),
        }
    }
}

impl<C> std::fmt::Debug for EagerFuture<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EagerFuture").finish_non_exhaustive()
    }
}

impl<C: ResultCell> EagerFuture<C> {
    /// Initialize the cell with the given `work` future, starting the work immediately on `handle`.
    pub fn new<Fut>(work: Fut, handle: &Handle) -> Self
    where
        Fut: Future<Output = C::Value> + Send + 'static,
    {
        let inner = Arc::new(Inner {
            cell: C::default(),
            ready: Notify::new(),
        });

        // Drive on a spawned task, so a `get` cancelled while waiting cannot strand the work.
        let driver = Arc::clone(&inner);
        let task = handle.spawn(async move {
            let value = work.await;
            driver.cell.fill(value);
            driver.ready.notify_waiters();
        });

        Self {
            inner,
            abort: task.abort_handle(),
        }
    }

    /// Fetch a clone of the value, awaiting the work if necessary.
    pub async fn get(&self) -> C::Value {
        if let Some(value) = self.inner.cell.peek() {
            return value;
        }

        loop {
            let notified = self.inner.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if let Some(value) = self.inner.cell.peek() {
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
    use std::time::Duration;

    use rstest::rstest;

    use super::{EagerFutureCell, MutEagerFutureCell};

    #[rstest]
    #[case(42)]
    #[case(7)]
    #[tokio::test]
    async fn computes_once_and_caches(#[case] value: usize) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let cell: EagerFutureCell<usize> = EagerFutureCell::new(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                value
            },
            &tokio::runtime::Handle::current(),
        );

        // Repeated gets return the cached value, and the work runs exactly once even though the
        // eager background kick and these gets can race.
        assert_eq!(cell.get().await, value);
        assert_eq!(cell.get().await, value);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn constructs_from_a_handle_outside_the_runtime() {
        // The explicit handle lets us construct (and eagerly spawn) from a thread that is not
        // itself running inside the runtime.
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cell: EagerFutureCell<usize> = EagerFutureCell::new(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                7usize
            },
            rt.handle(),
        );

        assert_eq!(rt.block_on(cell.get()), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn overwrite_supersedes_a_slow_future() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        let cell: MutEagerFutureCell<usize> = MutEagerFutureCell::new(
            async move {
                // Long enough that the `overwrite` below wins the race.
                tokio::time::sleep(Duration::from_secs(30)).await;
                counter.fetch_add(1, Ordering::SeqCst);
                1
            },
            &tokio::runtime::Handle::current(),
        );

        cell.overwrite(2);

        // The emplaced value is observed, and the aborted future never ran to completion.
        assert_eq!(cell.get().await, 2);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn overwrite_replaces_an_already_completed_future() {
        let cell: MutEagerFutureCell<usize> =
            MutEagerFutureCell::new(async move { 1 }, &tokio::runtime::Handle::current());

        // Let the eager future resolve first, then overwrite it.
        assert_eq!(cell.get().await, 1);
        cell.overwrite(2);
        assert_eq!(cell.get().await, 2);
    }
}
