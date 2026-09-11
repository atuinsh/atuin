use atuin_client::history::{CommandCapture, HistoryId};

use super::{CaptureError, DeleteOutputError, GetOutputError, Storage};

/// [`Storage`] implementation which does nothing. All output capture is discarded.
#[derive(Debug, Clone, Copy)]
pub struct NopStorage;

impl Storage for NopStorage {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Ok(())
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Ok(None)
    }

    async fn remove(&self, _ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        Ok(())
    }

    fn estimated_disk_space(&self) -> u64 {
        0
    }

    async fn all_ids(&self) -> Result<Vec<HistoryId>, GetOutputError> {
        Ok(Vec::new())
    }

    async fn eviction_candidates(
        &self,
        _reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        Ok(Vec::new())
    }
}
