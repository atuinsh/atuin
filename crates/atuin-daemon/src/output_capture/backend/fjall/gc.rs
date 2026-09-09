use std::time::Duration;

use atuin_common::units::{ByteSize, Percent};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::{Backend, FjallBackend};

pub struct Gc {
    task: JoinHandle<()>,
}

impl Gc {
    const INTERVAL: Duration = Duration::from_secs(60);
    const TRIGGER_FRACTION: Percent = Percent::new(90.0);
    const TARGET_FRACTION: Percent = Percent::new(80.0);

    pub fn spawn(backend: FjallBackend, budget: ByteSize) -> Self {
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                interval.tick().await;

                let size = match backend.logical_size().await {
                    Ok(size) => size,
                    Err(err) => {
                        tracing::warn!(?err, "output capture gc failed to measure size");
                        continue;
                    }
                };

                let trigger = (Self::TRIGGER_FRACTION * budget).bytes();
                if size < trigger {
                    continue;
                }

                let target = (Self::TARGET_FRACTION * budget).bytes();
                let reclaim = size.saturating_sub(target);

                let ids = match backend.oldest_ids_totaling(reclaim).await {
                    Ok(ids) => ids,
                    Err(err) => {
                        tracing::warn!(?err, "output capture gc failed to select entries");
                        continue;
                    }
                };
                if ids.is_empty() {
                    continue;
                }

                if let Err(err) = backend.remove(ids).await {
                    tracing::warn!(?err, "output capture gc failed to remove entries");
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
