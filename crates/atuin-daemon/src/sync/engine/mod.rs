//! The sync engine handle.

mod worker;

use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::daemon::DaemonHandle;
use crate::search::SearchIndex;

/// Owns the background sync loop.
///
/// Dropping this aborts the loop. An in-flight sync is interrupted rather than
/// drained; sync is incremental and idempotent, so it simply resumes on the next
/// launch.
#[derive(Debug)]
pub struct SyncEngine {
    task: JoinHandle<()>,
}

impl SyncEngine {
    /// Spawn the background sync loop.
    #[must_use]
    pub fn spawn(handle: DaemonHandle, index: Arc<RwLock<SearchIndex>>) -> Self {
        Self {
            task: tokio::spawn(worker::run(handle, index)),
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}
