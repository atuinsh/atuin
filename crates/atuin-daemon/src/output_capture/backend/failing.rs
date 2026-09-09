use atuin_client::history::{CommandCapture, HistoryId};

use super::{
    Backend, BackendError, CaptureError, DeleteOutputError, GetOutputError, OutputCaptureStats,
};

/// A [`Backend`] whose every operation fails, standing in for a broken output store.
///
/// It exists only to exercise the best-effort handling around the store -- most importantly that a
/// failed deletion never sinks the history delete itself. Production never selects it; only
/// [`OutputCapture::failing`](crate::OutputCapture::failing), a test hook, builds one.
#[derive(Debug, Clone, Copy)]
pub struct FailingBackend;

fn unavailable() -> BackendError {
    "the output-capture store is unavailable".into()
}

impl Backend for FailingBackend {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Err(CaptureError::Storage(unavailable()))
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Err(GetOutputError::Storage(unavailable()))
    }

    async fn remove(&self, _ids: Vec<HistoryId>) -> Result<(), DeleteOutputError> {
        Err(DeleteOutputError::Storage(unavailable()))
    }

    async fn stats(&self) -> Result<Option<OutputCaptureStats>, GetOutputError> {
        Err(GetOutputError::Storage("failing backend".into()))
    }
}
