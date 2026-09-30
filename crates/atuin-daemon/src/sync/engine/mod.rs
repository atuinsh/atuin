//! The sync engine handle.

mod worker;

use std::sync::Arc;

use atuin_client::ai_session::AiSessionDatabase;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use worker::Worker;

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
        ai_session_db: Option<AiSessionDatabase>,
        ai_session_recovery: Recovery,
    ) -> Self {
        Self {
            task: tokio::spawn(async {
                match Worker::new(handle, index, ai_session_db, ai_session_recovery).await {
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
