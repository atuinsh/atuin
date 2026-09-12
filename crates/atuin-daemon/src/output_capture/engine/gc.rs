//! Disk-budget garbage collection for the output capture backend.

use std::sync::Arc;
use std::time::Duration;

use atuin_common::units::{ByteSize, Percent};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::output_capture::{AnyOutputStore, OutputStoreOps};

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

    pub fn spawn(backend: Arc<AnyOutputStore>, budget: ByteSize) -> Self {
        let budget = budget.as_u64();
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                let size = backend.estimated_disk_space();

                let trigger = budget * Self::TRIGGER_SHARE;
                if size < trigger {
                    continue;
                }

                let target = budget * Self::TARGET_SHARE;
                let reclaim = size.saturating_sub(target);

                let victims = match backend.eviction_candidates(reclaim).await {
                    Ok(victims) => victims,
                    Err(err) => {
                        tracing::error!(?err, "failed to select entries to evict");
                        continue;
                    }
                };

                if let Err(err) = backend.remove(&victims).await {
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
