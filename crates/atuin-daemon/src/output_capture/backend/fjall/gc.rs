use std::time::Duration;

use atuin_common::units::ByteSize;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::FjallBackend;

pub struct Gc {
    task: JoinHandle<()>,
}

impl Gc {
    /// The period at which the garbage collector ticks, roughly.
    const INTERVAL: Duration = Duration::from_mins(1);

    /// The minimum fraction of the budget before we try to perform old-entry cleanup.
    const TRIGGER_FRACTION: f64 = 0.95;

    /// The target fraction of the budget. We'll trim any elements to fit this fraction.
    const TARGET_FRACTION: f64 = 0.9;

    pub fn spawn(backend: FjallBackend, budget: ByteSize) -> Self {
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                let size = backend.estimated_disk_space();

                let trigger = (Self::TRIGGER_FRACTION * budget).bytes();
                if size < trigger {
                    continue;
                }

                let target = (Self::TARGET_FRACTION * budget).bytes();
                let reclaim = size.saturating_sub(target);

                if let Err(err) = backend.reclaim(reclaim).await {
                    tracing::warn!(?err, "output capture gc failed to reclaim entries");
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
