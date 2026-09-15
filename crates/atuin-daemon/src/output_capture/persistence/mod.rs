mod blob;

use atuin_client::history::{CommandCapture, HistoryId};
#[cfg(test)]
pub use blob::FailingBlobStore;
pub use blob::{
    AnyBlobStore, BlobStore, CaptureError, DeleteOutputError, FjallBlobStore, GetOutputError,
    NopBlobStore,
};

/// A thin facade over the durable blob store.
///
/// Today it simply forwards to the [`BlobStore`]. It exists as the seam where the full-text search
/// index will be threaded alongside the blob store.
#[derive(Debug)]
pub struct OutputStore {
    blob: AnyBlobStore,
}

impl OutputStore {
    pub fn new(blob: AnyBlobStore) -> Self {
        Self { blob }
    }

    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        self.blob.capture(id, capture).await
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.blob.get(id).await
    }

    pub async fn remove(&self, ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        self.blob.remove(ids.iter().copied()).await
    }

    pub fn estimated_disk_space(&self) -> u64 {
        self.blob.estimated_disk_space()
    }

    pub async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.blob.eviction_candidates(reclaim_bytes).await
    }
}
