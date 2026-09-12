mod gc;
mod reconciler;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use gc::Gc;
use reconciler::Reconciler;
use tracing::{error, warn};

use super::persistence::{
    AnyBlobStore, AnyIndex, FjallBlobStore, NopBlobStore, NopIndex, OutputStore, SqliteIndex,
};
use super::{CaptureError, DeleteOutputError, GetOutputError};

#[derive(Debug)]
pub struct OutputCaptureEngine {
    store: Arc<OutputStore>,
    _gc: Option<Gc>,
    _reconciler: Option<Reconciler>,
}

impl OutputCaptureEngine {
    #[must_use]
    pub async fn open(path: impl AsRef<Path>, max_disk_usage: DiskUsageLimit) -> Self {
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

        // The index is derived, so a failure to open it leaves capture working; only search is lost.
        let store = match SqliteIndex::open(&index_path(path)).await {
            Ok(index) => OutputStore::new(AnyBlobStore::Fjall(storage), AnyIndex::Sqlite(index)),
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output search index; search over captured output is \
                     disabled"
                );
                OutputStore::new(AnyBlobStore::Fjall(storage), AnyIndex::Nop(NopIndex))
            }
        };
        let store = Arc::new(store);

        let reconciler = Reconciler::spawn(store.clone());

        let gc = match max_disk_usage.resolve_for_path(path) {
            Ok(budget) => budget.map(|budget| Gc::spawn(store.clone(), budget)),
            Err(err) => {
                warn!(?err, ?path, "failed to resolve the output capture disk budget; gc disabled");
                None
            }
        };

        Self {
            store,
            _gc: gc,
            _reconciler: Some(reconciler),
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self {
            store: Arc::new(OutputStore::new(
                AnyBlobStore::Nop(NopBlobStore),
                AnyIndex::Nop(NopIndex),
            )),
            _gc: None,
            _reconciler: None,
        }
    }

    /// A capture store whose every storage operation fails, standing in for a broken store.
    #[cfg(test)]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            store: Arc::new(OutputStore::new(
                AnyBlobStore::Failing(super::persistence::FailingBlobStore),
                AnyIndex::Nop(NopIndex),
            )),
            _gc: None,
            _reconciler: None,
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

    #[must_use]
    pub fn store(&self) -> Arc<OutputStore> {
        self.store.clone()
    }
}

/// The sqlite index lives beside the fjall store directory as a sibling file, so it never lands
/// among fjall's own files. `.../output-capture` becomes `.../output-capture-index.sqlite`.
fn index_path(fjall_dir: &Path) -> PathBuf {
    let mut name =
        fjall_dir.file_name().map_or_else(|| OsString::from("output-capture"), OsString::from);
    name.push("-index.sqlite");
    fjall_dir.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;
    use crate::output_capture::OutputMatch;

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

    async fn temp_store() -> (OutputCaptureEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            OutputCaptureEngine::open(dir.path().join("capture"), DiskUsageLimit::Unlimited).await;
        (store, dir)
    }

    async fn search_hits(store: &OutputStore, query: &str, limit: usize) -> Vec<OutputMatch> {
        store.search(query, limit).await.try_collect().await.expect("search")
    }

    #[tokio::test]
    async fn open_uses_the_fjall_backend_when_the_path_is_usable() {
        let (store, _dir) = temp_store().await;
        store.capture(hid(1), cap("hello")).await.expect("capture");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "hello");
        assert_eq!(search_hits(&store.store(), "hello", 10).await.len(), 1);
    }

    #[tokio::test]
    async fn open_falls_back_to_nop_when_fjall_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("occupied");
        std::fs::write(&path, b"not a database").expect("write file");

        let store = OutputCaptureEngine::open(&path, DiskUsageLimit::Unlimited).await;
        store.capture(hid(1), cap("hello")).await.expect("capture is discarded, not failed");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn open_keeps_capturing_when_only_the_index_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture");
        // Occupy the index's path with a directory so sqlite cannot open it as a file.
        std::fs::create_dir_all(index_path(&path)).expect("occupy index path");

        let store = OutputCaptureEngine::open(&path, DiskUsageLimit::Unlimited).await;
        store.capture(hid(1), cap("stored but unsearchable")).await.expect("capture");
        assert!(store.get(hid(1)).await.expect("get").is_some());
        assert!(search_hits(&store.store(), "unsearchable", 10).await.is_empty());
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
        let (store, _dir) = temp_store().await;
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove([hid(1)]).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_on_the_nop_backend_is_ok() {
        let store = OutputCaptureEngine::nop();
        store.remove([hid(1), hid(2)]).await.expect("remove is discarded, not failed");
    }

    #[tokio::test]
    async fn searcher_sees_what_the_store_captures() {
        let (store, _dir) = temp_store().await;
        let searcher = store.store();
        store.capture(hid(1), cap("compilation error: missing semicolon")).await.expect("capture");

        let hits = search_hits(&searcher, "semicolon", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));

        store.remove([hid(1)]).await.expect("remove");
        assert!(search_hits(&searcher, "semicolon", 10).await.is_empty());
    }

    #[tokio::test]
    async fn search_on_a_nop_store_is_empty() {
        let store = OutputCaptureEngine::nop();
        store.capture(hid(1), cap("nothing is indexed here")).await.expect("capture");
        assert!(search_hits(&store.store(), "nothing", 10).await.is_empty());
    }
}
