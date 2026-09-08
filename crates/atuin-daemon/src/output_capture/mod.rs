mod backend;
mod schema;

use atuin_client::history::{CommandCapture, HistoryId};
use backend::{AnyBackend, Backend as _, FjallBackend, NopBackend};
pub use backend::{BackendKind, CaptureError, GetOutputError};
use tracing::error;

/// [`OutputCapture`] is the core engine responsible for collecting command output.
#[derive(derive_more::Debug)]
pub struct OutputCapture {
    backend: AnyBackend,
}

impl OutputCapture {
    #[must_use]
    pub fn open(path: impl AsRef<std::path::Path>) -> Self {
        let path = path.as_ref();
        match FjallBackend::open(path) {
            Ok(backend) => Self {
                backend: AnyBackend::Fjall(backend),
            },
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output capture store; output capture is disabled"
                );
                Self::nop()
            }
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self {
            backend: AnyBackend::Nop(NopBackend),
        }
    }

    #[must_use]
    pub fn kind(&self) -> BackendKind {
        self.backend.kind()
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

#[cfg(test)]
mod tests {
    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*Uuid::from_u128(n).as_bytes())
    }

    fn cap(output: &str) -> CommandCapture {
        CommandCapture {
            output: output.to_string(),
            output_observed_bytes: u64::conv(output.len()),
            output_truncated: false,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[tokio::test]
    async fn open_uses_the_fjall_backend_when_the_path_is_usable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = OutputCapture::open(dir.path().join("capture"));
        assert_eq!(store.kind(), BackendKind::Fjall);
        store.capture(hid(1), cap("hello")).await.expect("capture");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output, "hello");
    }

    #[tokio::test]
    async fn open_falls_back_to_nop_when_fjall_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("occupied");
        std::fs::write(&path, b"not a database").expect("write file");

        let store = OutputCapture::open(&path);
        assert_eq!(store.kind(), BackendKind::Nop);
        store.capture(hid(1), cap("hello")).await.expect("capture is discarded, not failed");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn nop_constructor_discards_everything() {
        let store = OutputCapture::nop();
        assert_eq!(store.kind(), BackendKind::Nop);
        store.capture(hid(1), cap("first")).await.expect("first");
        store.capture(hid(1), cap("second")).await.expect("second");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }
}
