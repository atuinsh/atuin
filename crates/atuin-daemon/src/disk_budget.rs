//! Keeping a store's disk usage under a budget, by deleting its oldest entries.

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::units::{ByteSize, Percent};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

/// A store that a [`Gc`] keeps under a budget.
pub trait Evict: Send + Sync + 'static {
    type Error: Debug + Send;

    /// The bytes that the store uses, by the store's own running estimate.
    ///
    /// The estimate must shrink as soon as an eviction returns. The disk usage that the
    /// filesystem reports lags behind deletions, and a [`Gc`] that read it would evict the same
    /// space again.
    fn estimated_disk_space(&self) -> u64;

    /// Deletes the oldest entries until they add up to at least `bytes`, or until nothing that
    /// can be deleted is left.
    fn evict(&self, bytes: u64) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// A task that keeps a store under a budget for as long as the `Gc` lives.
#[derive(Debug)]
pub struct Gc {
    task: JoinHandle<()>,
}

impl Gc {
    /// The period at which the garbage collector ticks, roughly.
    const INTERVAL: Duration = Duration::from_mins(1);

    /// The minimum share of the budget before we try to perform old-entry cleanup.
    const TRIGGER_SHARE: Percent = Percent::new(95.0);

    /// The share of the budget we trim down to once cleanup runs.
    const TARGET_SHARE: Percent = Percent::new(90.0);

    pub fn spawn<S: Evict>(store: Arc<S>, budget: ByteSize) -> Self {
        let budget = budget.as_u64();
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                let size = store.estimated_disk_space();

                let trigger = budget * Self::TRIGGER_SHARE;
                if size < trigger {
                    continue;
                }

                let target = budget * Self::TARGET_SHARE;
                let reclaim = size.saturating_sub(target);

                if let Err(err) = store.evict(reclaim).await {
                    tracing::error!(?err, "failed to evict entries");
                }
            }
        });

        Self { task }
    }
}

impl Drop for Gc {
    fn drop(&mut self) {
        self.task.abort();
    }
}
