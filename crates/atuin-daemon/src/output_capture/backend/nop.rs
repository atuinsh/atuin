use atuin_client::history::{CommandCapture, HistoryId};

use super::{Backend, CaptureError, DeleteOutputError, GetOutputError, OutputCaptureStats};

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

    async fn stats(&self) -> Result<Option<OutputCaptureStats>, GetOutputError> {
        Ok(None)
    }
}
