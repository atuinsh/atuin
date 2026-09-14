use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::futures::stream::ChunkedStream;

use super::{BlobStore, CaptureError, DeleteOutputError, GetOutputError};

/// [`BlobStore`] implementation which does nothing. All output capture is discarded.
#[derive(Debug, Clone, Copy)]
pub struct NopBlobStore;

impl BlobStore for NopBlobStore {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Ok(())
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Ok(None)
    }

    async fn remove(&self, _ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError> {
        Ok(())
    }

    fn estimated_disk_space(&self) -> u64 {
        0
    }

    async fn all_ids(&self) -> ChunkedStream<Result<HistoryId, GetOutputError>> {
        ChunkedStream::empty()
    }

    async fn eviction_candidates(
        &self,
        _reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        Ok(Vec::new())
    }
}
