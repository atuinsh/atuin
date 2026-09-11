//! Cloud sync.
//!
//! A background task that periodically synchronizes local history with the Atuin
//! cloud server and feeds freshly-downloaded entries into the search index.

mod engine;

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
pub struct Sync {
    task: JoinHandle<()>,
}

impl Sync {
    /// Spawn the background sync loop.
    #[must_use]
    pub fn spawn(handle: DaemonHandle, index: Arc<RwLock<SearchIndex>>) -> Self {
        Self {
            task: tokio::spawn(engine::run(handle, index)),
        }
    }
}

impl Drop for Sync {
    fn drop(&mut self) {
        self.task.abort();
    }
}
