mod backend;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use atuin_common::string::highlighted::HighlightedString;
pub use backend::{
    AnyOutputStore, CaptureError, DeleteOutputError, GetOutputError, OutputStoreKind,
    OutputStoreOps,
};
use backend::{FjallStorage, Gc, NopIndex, OutputStore, SqliteIndex};
use tokio::task::JoinHandle;
use tracing::{error, warn};

#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    pub output: HighlightedString,
    pub score: f64,
}

/// [`OutputCaptureEngine`] is the core engine responsible for collecting command output.
///
/// It owns the store (storage plus its search index) and the background tasks that keep it
/// healthy: the gc enforcing the disk budget and the boot-time reconcile. A read-only
/// [`searcher`](Self::searcher) is handed to the search service to serve queries.
#[derive(Debug)]
pub struct OutputCaptureEngine {
    store: Arc<AnyOutputStore>,
    // Held only to abort their background tasks on drop; never read.
    _gc: Option<Gc>,
    reconcile_task: Option<JoinHandle<()>>,
}

impl OutputCaptureEngine {
    #[must_use]
    pub async fn open(path: impl AsRef<Path>, max_disk_usage: DiskUsageLimit) -> Self {
        let path = path.as_ref();

        let storage = match FjallStorage::open(path) {
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
            Ok(index) => AnyOutputStore::Fjall(OutputStore::new(storage, index)),
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output search index; search over captured output is \
                     disabled"
                );
                AnyOutputStore::FjallUnindexed(OutputStore::new(storage, NopIndex))
            }
        };
        let store = Arc::new(store);

        // Reconcile in the background so boot isn't blocked by a large first-time index build.
        let reconcile_task = {
            let store = store.clone();
            tokio::spawn(async move {
                if let Err(err) = store.reconcile().await {
                    warn!(?err, "failed to reconcile the output search index against the store");
                }
            })
        };

        // The gc drives the store from a background task, holding only a clone of the `Arc`; the
        // store points at no task, so that clone forms no cycle that would keep the task alive.
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
            reconcile_task: Some(reconcile_task),
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self::without_tasks(AnyOutputStore::Nop(OutputStore::new(backend::NopStorage, NopIndex)))
    }

    /// A capture store whose every storage operation fails, standing in for a broken store.
    ///
    /// Lets a test prove that a broken output store never sinks a primary operation (for example,
    /// that deleting history still succeeds when its captured output cannot be removed). Paired with
    /// a nop index so the failure under test is the storage's.
    #[cfg(test)]
    #[must_use]
    pub fn failing() -> Self {
        Self::without_tasks(AnyOutputStore::Failing(OutputStore::new(
            backend::FailingStorage,
            NopIndex,
        )))
    }

    fn without_tasks(store: AnyOutputStore) -> Self {
        Self {
            store: Arc::new(store),
            _gc: None,
            reconcile_task: None,
        }
    }

    #[must_use]
    pub fn kind(&self) -> OutputStoreKind {
        OutputStoreKind::from(&*self.store)
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
    pub fn store(&self) -> Arc<AnyOutputStore> {
        self.store.clone()
    }
}

impl Drop for OutputCaptureEngine {
    fn drop(&mut self) {
        if let Some(task) = self.reconcile_task.take() {
            task.abort();
        }
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
    use futures::TryStreamExt;
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

    async fn temp_store() -> (OutputCaptureEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            OutputCaptureEngine::open(dir.path().join("capture"), DiskUsageLimit::Unlimited).await;
        (store, dir)
    }

    async fn search_hits(store: &AnyOutputStore, query: &str, limit: usize) -> Vec<OutputMatch> {
        store.search(query, limit).await.items().try_collect().await.expect("search")
    }

    #[tokio::test]
    async fn open_uses_the_fjall_backend_when_the_path_is_usable() {
        let (store, _dir) = temp_store().await;
        assert_eq!(store.kind(), OutputStoreKind::Fjall);
        store.capture(hid(1), cap("hello")).await.expect("capture");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "hello");
    }

    #[tokio::test]
    async fn open_falls_back_to_nop_when_fjall_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("occupied");
        std::fs::write(&path, b"not a database").expect("write file");

        let store = OutputCaptureEngine::open(&path, DiskUsageLimit::Unlimited).await;
        assert_eq!(store.kind(), OutputStoreKind::Nop);
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
        assert_eq!(store.kind(), OutputStoreKind::FjallUnindexed);
        store.capture(hid(1), cap("stored but unsearchable")).await.expect("capture");
        assert!(store.get(hid(1)).await.expect("get").is_some());
        assert!(search_hits(&store.store(), "unsearchable", 10).await.is_empty());
    }

    #[tokio::test]
    async fn nop_constructor_discards_everything() {
        let store = OutputCaptureEngine::nop();
        assert_eq!(store.kind(), OutputStoreKind::Nop);
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
