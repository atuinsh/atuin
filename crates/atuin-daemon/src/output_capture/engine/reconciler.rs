//! Background task which reconciles the index against the actual output store.
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::warn;

use crate::output_capture::{AnyOutputStore, OutputStoreOps};

#[derive(Debug)]
pub struct Reconciler {
    task: JoinHandle<()>,
}

impl Reconciler {
    const INTERVAL: Duration = Duration::from_mins(15);

    pub fn spawn(store: Arc<AnyOutputStore>) -> Self {
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                if let Err(err) = store.reconcile().await {
                    warn!(?err, "failed to reconcile the output search index against the store");
                }
            }
        });

        Self { task }
    }
}

impl Drop for Reconciler {
    fn drop(&mut self) {
        self.task.abort();
    }
}
