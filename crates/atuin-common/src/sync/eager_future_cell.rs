use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;
use tokio::sync::{Notify, OnceCell};
use tokio::task::AbortHandle;

/// The storage backing an [`EagerFuture`]: a slot that starts empty and later holds the future's
/// output. Implemented for [`OnceCell<T>`] (write-once) and [`Mutex<Option<T>>`] (overwritable).
pub trait ResultCell: Send + Sync + 'static {
    /// The value the cell holds.
    type Value: Clone + Send + Sync + 'static;

    /// A fresh, empty slot.
    fn empty() -> Self;

    /// Store the value produced by the eager future. A slot that already holds a value keeps it --
    /// that is what lets [`EagerFuture::emplace_cancelling`] win the race against a finishing future.
    fn fill(&self, value: Self::Value);

    /// A clone of the stored value, if the slot has been filled.
    fn peek(&self) -> Option<Self::Value>;
}

impl<T: Clone + Send + Sync + 'static> ResultCell for OnceCell<T> {
    type Value = T;

    fn empty() -> Self {
        Self::new()
    }

    fn fill(&self, value: T) {
        let _ = self.set(value);
    }

    fn peek(&self) -> Option<T> {
        self.get().cloned()
    }
}

impl<T: Clone + Send + Sync + 'static> ResultCell for Mutex<Option<T>> {
    type Value = T;

    fn empty() -> Self {
        Self::new(None)
    }

    fn fill(&self, value: T) {
        let mut slot = self.lock().unwrap();
        // Keep an existing value: an `emplace_cancelling` may have already won.
        if slot.is_none() {
            *slot = Some(value);
        }
    }

    fn peek(&self) -> Option<T> {
        self.lock().unwrap().clone()
    }
}

#[derive(Debug)]
struct Inner<C> {
    /// The computed value, once it is ready.
    cell: C,
    /// Notified once [`Self::cell`] is ready, waking anything waiting in [`EagerFuture::get`].
    ready: Notify,
}

/// A cell whose value is produced exactly once, eagerly, in the background, over pluggable storage
/// `C` (see [`ResultCell`]).
///
/// [`EagerFuture::new`] accepts a future which is executed in the background. [`EagerFuture::get`]
/// either waits for the future to produce the value, or returns the already-stored value.
///
/// Use the [`EagerFutureCell`] alias for a write-once slot, or [`MutEagerFutureCell`] for one that
/// additionally supports [`EagerFuture::emplace_cancelling`].
pub struct EagerFuture<C> {
    inner: Arc<Inner<C>>,
    /// Aborts the background task, used by [`EagerFuture::emplace_cancelling`].
    abort: AbortHandle,
}

/// A write-once [`EagerFuture`]: the background future's value is the only value it ever holds.
pub type EagerFutureCell<T> = EagerFuture<OnceCell<T>>;

/// An [`EagerFuture`] whose value can be overwritten via [`EagerFuture::emplace_cancelling`].
pub type MutEagerFutureCell<T> = EagerFuture<Mutex<Option<T>>>;

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
            cell: C::empty(),
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

impl<T: Clone + Send + Sync + 'static> MutEagerFutureCell<T> {
    /// Force `value` into the cell, aborting the background future so its (now-superseded) result is
    /// discarded. Any current or future [`get`](EagerFuture::get) observes `value`.
    pub fn emplace_cancelling(&self, value: T) {
        // Abort first so a task sitting between `work.await` and `fill` cannot re-fill afterwards;
        // even if it does slip in, `fill` keeps our value because the slot is already `Some`.
        self.abort.abort();
        *self.inner.cell.lock().unwrap() = Some(value);
        self.inner.ready.notify_waiters();
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
    async fn emplace_cancelling_supersedes_a_slow_future() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        let cell: MutEagerFutureCell<usize> = MutEagerFutureCell::new(
            async move {
                // Long enough that `emplace_cancelling` below wins the race.
                tokio::time::sleep(Duration::from_secs(30)).await;
                counter.fetch_add(1, Ordering::SeqCst);
                1
            },
            &tokio::runtime::Handle::current(),
        );

        cell.emplace_cancelling(2);

        // The emplaced value is observed, and the aborted future never ran to completion.
        assert_eq!(cell.get().await, 2);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn emplace_cancelling_overrides_an_already_completed_future() {
        let cell: MutEagerFutureCell<usize> =
            MutEagerFutureCell::new(async move { 1 }, &tokio::runtime::Handle::current());

        // Let the eager future resolve first, then overwrite it.
        assert_eq!(cell.get().await, 1);
        cell.emplace_cancelling(2);
        assert_eq!(cell.get().await, 2);
    }
}
