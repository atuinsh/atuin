use atuin_client::history::{CommandCapture, HistoryId};

use super::{CaptureError, DeleteOutputError, GetOutputError, Storage, StorageError};

/// A [`Storage`] whose every operation fails, standing in for a broken output store.
///
/// It exists only to exercise the best-effort handling around the store -- most importantly that a
/// failed deletion never sinks the history delete itself. Production never selects it; only
/// [`OutputCapture::failing`](crate::OutputCapture::failing), a test hook, builds one.
#[derive(Debug, Clone, Copy)]
pub struct FailingStorage;

fn unavailable() -> StorageError {
    "the output-capture store is unavailable".into()
}

impl Storage for FailingStorage {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Err(CaptureError::Storage(unavailable()))
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Err(GetOutputError::Storage(unavailable()))
    }

    async fn remove(&self, _ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        Err(DeleteOutputError::Storage(unavailable()))
    }

    fn estimated_disk_space(&self) -> u64 {
        0
    }

    async fn all_ids(&self) -> Result<Vec<HistoryId>, GetOutputError> {
        Err(GetOutputError::Storage(unavailable()))
    }

    async fn eviction_candidates(
        &self,
        _reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        Err(DeleteOutputError::Storage(unavailable()))
    }
}
