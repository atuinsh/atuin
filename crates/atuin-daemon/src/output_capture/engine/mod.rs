mod gc;

use std::path::Path;
use std::sync::Arc;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use gc::Gc;
use tracing::{error, warn};

use super::persistence::{AnyBlobStore, FjallBlobStore, NopBlobStore, OutputStore};
use super::{CaptureError, DeleteOutputError, GetOutputError};

#[derive(Debug)]
pub struct OutputCaptureEngine {
    store: Arc<OutputStore>,
    _gc: Option<Gc>,
}

impl OutputCaptureEngine {
    #[must_use]
    pub fn open(path: impl AsRef<Path>, max_disk_usage: DiskUsageLimit) -> Self {
        let path = path.as_ref();

        let storage = match FjallBlobStore::open(path) {
            Ok(storage) => storage,
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output capture store; output capture is disabled"
                );
                return Self::nop();
            }
        };
        let store = Arc::new(OutputStore::new(AnyBlobStore::Fjall(storage)));

        let gc = match max_disk_usage.resolve_for_path(path) {
            Ok(budget) => budget.map(|budget| Gc::spawn(store.clone(), budget)),
            Err(err) => {
                warn!(?err, ?path, "failed to resolve the output capture disk budget; gc disabled");
                None
            }
        };

        Self { store, _gc: gc }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self {
            store: Arc::new(OutputStore::new(AnyBlobStore::Nop(NopBlobStore))),
            _gc: None,
        }
    }

    /// A capture store whose every storage operation fails, standing in for a broken store.
    #[cfg(test)]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            store: Arc::new(OutputStore::new(AnyBlobStore::Failing(
                super::persistence::FailingBlobStore,
            ))),
            _gc: None,
        }
    }

    /// Capture a command and associate it with the given history id.
    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        self.store.capture(id, capture).await
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.store.get(id).await
    }

    /// Forget the captured output of every history id in `ids`.
    ///
    /// Removing an absent id is a no-op, so this is safe to call for a batch that mixes captured and
    /// never-captured ids.
    pub async fn remove(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<(), DeleteOutputError> {
        let ids: Vec<HistoryId> = ids.into_iter().collect();
        self.store.remove(&ids).await
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
            output_start: output.to_string(),
            output_end: None,
            output_observed_bytes: u64::conv(output.len()),
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    fn temp_store() -> (OutputCaptureEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            OutputCaptureEngine::open(dir.path().join("capture"), DiskUsageLimit::Unlimited);
        (store, dir)
    }

    #[tokio::test]
    async fn open_uses_the_fjall_backend_when_the_path_is_usable() {
        let (store, _dir) = temp_store();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "hello");
    }

    #[tokio::test]
    async fn open_falls_back_to_nop_when_fjall_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("occupied");
        std::fs::write(&path, b"not a database").expect("write file");

        let store = OutputCaptureEngine::open(&path, DiskUsageLimit::Unlimited);
        store.capture(hid(1), cap("hello")).await.expect("capture is discarded, not failed");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn nop_constructor_discards_everything() {
        let store = OutputCaptureEngine::nop();
        store.capture(hid(1), cap("first")).await.expect("first");
        store.capture(hid(1), cap("second")).await.expect("second");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_forgets_captured_output() {
        let (store, _dir) = temp_store();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove([hid(1)]).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_on_the_nop_backend_is_ok() {
        let store = OutputCaptureEngine::nop();
        store.remove([hid(1), hid(2)]).await.expect("remove is discarded, not failed");
    }
}
