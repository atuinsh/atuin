//! Persists the buffered writes of a fjall store in the background.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::error;

/// Marks a store that holds commits that have not been persisted yet.
///
/// A writer marks the flag after each commit with Release ordering, and the flusher clears the
/// flag with Acquire ordering before it persists. When the flusher reads a mark, the commit
/// before that mark therefore happens before the persist, so the persist covers the commit.
/// With Relaxed ordering, the persist could miss the commit's writes. Only fjall's internal
/// locks might order them, and the store must not rely on fjall's internals.
#[derive(Debug, Default)]
pub struct DirtyFlag(AtomicBool);

impl DirtyFlag {
    /// Marks the store after a commit.
    pub fn mark(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a commit happened since the last persist.
    pub fn is_marked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Clears the flag, and then runs `persist` when the flag was marked. A failed persist
    /// marks the flag again, so that the next call tries again.
    pub fn persist_if_marked(
        &self,
        persist: impl FnOnce() -> fjall::Result<()>,
    ) -> fjall::Result<()> {
        if !self.0.swap(false, Ordering::Acquire) {
            return Ok(());
        }
        persist().inspect_err(|_| self.0.store(true, Ordering::Relaxed))
    }
}

/// A store that a [`Flusher`] persists.
pub trait Persist: Send + Sync + 'static {
    /// The flag that the store marks after each commit.
    fn dirty(&self) -> &DirtyFlag;

    /// Persists every commit so far.
    fn persist(&self) -> fjall::Result<()>;
}

/// A task that persists a store's commits every few seconds.
#[derive(Debug)]
pub struct Flusher {
    task: JoinHandle<()>,
    #[cfg_attr(
        not(feature = "desktop-capture"),
        expect(dead_code, reason = "only desktop capture stops its flusher")
    )]
    stop: Arc<Notify>,
}

impl Flusher {
    /// How often to persist. A persist takes about 1 to 10 ms.
    const SYNC_INTERVAL: Duration = Duration::from_secs(5);

    /// Spawns the task, which holds the store weakly between persists and ends once the store
    /// is gone. `name` names the store in the log.
    ///
    /// During a persist, the task holds the store. [`Flusher::stop`] waits for that persist to
    /// end.
    pub fn spawn<T: Persist>(store: Weak<T>, name: &'static str) -> Self {
        let stop = Arc::new(Notify::new());
        let stopped = Arc::clone(&stop);
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::SYNC_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    () = stopped.notified() => return,
                }
                let Some(store) = store.upgrade() else {
                    return;
                };
                if !store.dirty().is_marked() {
                    continue;
                }
                let persisted = tokio::task::spawn_blocking(move || {
                    store.dirty().persist_if_marked(|| store.persist())
                })
                .await
                .expect("a persist task panicked");
                if let Err(err) = persisted {
                    error!(?err, store = name, "could not persist a store, and will try again");
                }
            }
        });
        Self { task, stop }
    }

    /// Ends the task after its current persist, if any, and waits for it to end.
    #[cfg_attr(
        not(feature = "desktop-capture"),
        expect(dead_code, reason = "only desktop capture stops its flusher")
    )]
    pub async fn stop(mut self) {
        self.stop.notify_one();
        if let Err(err) = (&mut self.task).await {
            error!(?err, "a flusher failed");
        }
    }
}

impl Drop for Flusher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn a_failed_persist_leaves_the_flag_marked() {
        let flag = DirtyFlag::default();
        let mut persists = 0;
        flag.persist_if_marked(|| {
            persists += 1;
            Ok(())
        })
        .expect("nothing to persist");
        assert_eq!(persists, 0);

        flag.mark();
        let failed = flag.persist_if_marked(|| Err(fjall::Error::Poisoned));
        assert!(failed.is_err());
        assert!(flag.is_marked());

        flag.persist_if_marked(|| {
            persists += 1;
            Ok(())
        })
        .expect("persisted");
        assert_eq!(persists, 1);
        assert!(!flag.is_marked());
    }
}
