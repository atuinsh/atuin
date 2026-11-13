//! Synchronization primitives.

mod blocking_pool;
mod eager_future_cell;
mod sharded_mutex;

pub use blocking_pool::{BlockingCancelled, BlockingPool};
pub use eager_future_cell::{EagerFuture, EagerFutureCell, MutEagerFutureCell, ResultCell};
pub use sharded_mutex::{AsyncShardedMutex, ShardedMutex};

/// Drop `value` on tokio's blocking pool when there is a runtime. Without a runtime, `value` is
/// dropped before this returns.
///
/// Freeing a large value takes time. Freeing it on the async runtime would hold up other tasks.
pub fn free_in_background<T: Send + 'static>(value: T) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(move || drop(value));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread::{self, ThreadId};

    use rstest::rstest;

    use super::*;

    /// Sends the id of the thread that drops it.
    struct DropThread(mpsc::Sender<ThreadId>);

    impl Drop for DropThread {
        fn drop(&mut self) {
            self.0.send(thread::current().id()).unwrap();
        }
    }

    #[rstest]
    #[tokio::test]
    async fn frees_on_another_thread_with_a_runtime() {
        let (sender, receiver) = mpsc::channel();
        free_in_background(DropThread(sender));
        let dropped_on =
            tokio::task::spawn_blocking(move || receiver.recv().unwrap()).await.unwrap();
        assert_ne!(dropped_on, thread::current().id());
    }

    #[rstest]
    fn frees_before_returning_without_a_runtime() {
        let (sender, receiver) = mpsc::channel();
        free_in_background(DropThread(sender));
        assert_eq!(receiver.try_recv(), Ok(thread::current().id()));
    }
}
