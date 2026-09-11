use atuin_client::history::{CommandCapture, HistoryId};

use super::{Backend, CaptureError, DeleteOutputError, GetOutputError};

/// [`Backend`] implementation which does nothing. All output capture is discarded.
#[derive(Debug, Clone, Copy)]
pub struct NopBackend;

impl Backend for NopBackend {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Ok(())
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Ok(None)
    }

    async fn remove(&self, _ids: Vec<HistoryId>) -> Result<(), DeleteOutputError> {
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
