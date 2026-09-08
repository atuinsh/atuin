mod backend;
mod schema;

use atuin_client::history::{CommandCapture, HistoryId};

use backend::{Backend as _, FjallBackend};
pub use backend::{CaptureError, GetOutputError};

/// [`OutputCapture`] is the core engine responsible for collecting command output.
#[derive(derive_more::Debug)]
pub struct OutputCapture {
    backend: FjallBackend,
}

impl OutputCapture {
    /// Open (or create) the store at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> fjall::Result<Self> {
        Ok(Self { backend: FjallBackend::open(path)? })
    }

    /// Capture a command and associate it with the given history id.
    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        self.backend.capture(id, capture).await
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.backend.get(id).await
    }
}
