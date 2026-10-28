//! The sync engine handle.

mod rebuild;
mod worker;

use std::sync::Arc;

use atuin_client::si_session::SiSessionDatabase;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use worker::Worker;
#[cfg(test)]
pub use worker::spawn_si_session_projector;

use crate::daemon::DaemonHandle;
use crate::search::SearchIndex;
use crate::session_capture::Recovery;

/// Owns the background sync task.
#[derive(Debug)]
pub struct SyncEngine {
    task: JoinHandle<()>,
}

impl SyncEngine {
    /// Spawn the background sync loop.
    #[must_use]
    pub fn spawn(
        handle: DaemonHandle,
        index: Arc<RwLock<SearchIndex>>,
        si_session_db: Option<SiSessionDatabase>,
        si_session_recovery: Recovery,
    ) -> Self {
        Self {
            task: tokio::spawn(async {
                match Worker::new(handle, index, si_session_db, si_session_recovery).await {
                    Ok(worker) => worker.run().await,
                    Err(e) => tracing::error!("sync disabled: {e}"),
                }
            }),
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}
