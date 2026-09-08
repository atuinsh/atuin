use atuin_client::history::{CommandCapture, HistoryId};

use super::{Backend, CaptureError, GetOutputError};

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
}
