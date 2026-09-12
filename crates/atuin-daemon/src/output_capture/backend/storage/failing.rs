use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::futures::stream::ChunkedStream;

use super::{CaptureError, DeleteOutputError, GetOutputError, Storage, StorageError};

/// A [`Storage`] whose every operation fails, standing in for a broken output store.
///
/// This is for tests.
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

    async fn remove(&self, _ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError> {
        Err(DeleteOutputError::Storage(unavailable()))
    }

    fn estimated_disk_space(&self) -> u64 {
        0
    }

    async fn all_ids(&self) -> ChunkedStream<Result<HistoryId, GetOutputError>> {
        ChunkedStream::from_chunks([vec![Err(GetOutputError::Storage(unavailable()))]])
    }

    async fn eviction_candidates(
        &self,
        _reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        Err(DeleteOutputError::Storage(unavailable()))
    }
}
