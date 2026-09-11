mod backend;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use backend::{AnyBackend, Backend, FjallStorage, Gc, NopIndex, OutputBackend, SqliteIndex};
pub use backend::{
    BackendKind, CaptureError, DeleteOutputError, GetOutputError, IndexError, OutputMatch,
};
use tokio::task::JoinHandle;
use tracing::{error, warn};

/// [`OutputCapture`] is the core engine responsible for collecting command output.
///
/// It owns the backend (storage plus its search index) and the background tasks that keep it
/// healthy: the gc enforcing the disk budget and the boot-time reconcile. A read-only
/// [`searcher`](Self::searcher) is handed to the search service to serve queries.
#[derive(Debug)]
pub struct OutputCapture {
    backend: Arc<AnyBackend>,
    // Held only to abort their background tasks on drop; never read.
    _gc: Option<Gc>,
    reconcile_task: Option<JoinHandle<()>>,
}

impl OutputCapture {
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
        let backend = match SqliteIndex::open(&index_path(path)).await {
            Ok(index) => AnyBackend::Fjall(Backend::new(storage, index)),
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output search index; search over captured output is \
                     disabled"
                );
                AnyBackend::FjallUnindexed(Backend::new(storage, NopIndex))
            }
        };
        let backend = Arc::new(backend);

        // Reconcile in the background so boot isn't blocked by a large first-time index build.
        let reconcile_task = {
            let backend = backend.clone();
            tokio::spawn(async move {
                if let Err(err) = backend.reconcile().await {
                    warn!(?err, "failed to reconcile the output search index against the store");
                }
            })
        };

        // The gc drives the backend from a background task, holding only a clone of the `Arc`; the
        // backend points at no task, so that clone forms no cycle that would keep the task alive.
        let gc = match max_disk_usage.resolve_for_path(path) {
            Ok(budget) => budget.map(|budget| Gc::spawn(backend.clone(), budget)),
            Err(err) => {
                warn!(?err, ?path, "failed to resolve the output capture disk budget; gc disabled");
                None
            }
        };

        Self {
            backend,
            _gc: gc,
            reconcile_task: Some(reconcile_task),
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self::without_tasks(AnyBackend::Nop(Backend::new(backend::NopStorage, NopIndex)))
    }

    /// A capture store whose every storage operation fails, standing in for a broken store.
    ///
    /// Lets a test prove that a broken output store never sinks a primary operation (for example,
    /// that deleting history still succeeds when its captured output cannot be removed). Paired with
    /// a nop index so the failure under test is the storage's.
    #[cfg(test)]
    #[must_use]
    pub fn failing() -> Self {
        Self::without_tasks(AnyBackend::Failing(Backend::new(backend::FailingStorage, NopIndex)))
    }

    fn without_tasks(backend: AnyBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            _gc: None,
            reconcile_task: None,
        }
    }

    #[must_use]
    pub fn kind(&self) -> BackendKind {
        BackendKind::from(&*self.backend)
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

    /// Forget the captured output of every history id in `ids`.
    ///
    /// Removing an absent id is a no-op, so this is safe to call for a batch that mixes captured and
    /// never-captured ids.
    pub async fn remove(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<(), DeleteOutputError> {
        let ids: Vec<HistoryId> = ids.into_iter().collect();
        self.backend.remove(&ids).await
    }

    /// A read-only handle for querying captured output, for the search service.
    #[must_use]
    pub fn searcher(&self) -> OutputSearcher {
        OutputSearcher {
            backend: self.backend.clone(),
        }
    }
}

impl Drop for OutputCapture {
    fn drop(&mut self) {
        if let Some(task) = self.reconcile_task.take() {
            task.abort();
        }
    }
}

/// Search over captured output, and nothing else: the one part of [`OutputCapture`] the search
/// service gets to hold.
#[derive(Debug, Clone)]
pub struct OutputSearcher {
    backend: Arc<AnyBackend>,
}

impl OutputSearcher {
    /// Relevance-ranked full-text matches over captured output, most relevant first.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError> {
        self.backend.search(query, limit).await
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

    async fn temp_store() -> (OutputCapture, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            OutputCapture::open(dir.path().join("capture"), DiskUsageLimit::Unlimited).await;
        (store, dir)
    }

    #[tokio::test]
    async fn open_uses_the_fjall_backend_when_the_path_is_usable() {
        let (store, _dir) = temp_store().await;
        assert_eq!(store.kind(), BackendKind::Fjall);
        store.capture(hid(1), cap("hello")).await.expect("capture");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "hello");
    }

    #[tokio::test]
    async fn open_falls_back_to_nop_when_fjall_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("occupied");
        std::fs::write(&path, b"not a database").expect("write file");

        let store = OutputCapture::open(&path, DiskUsageLimit::Unlimited).await;
        assert_eq!(store.kind(), BackendKind::Nop);
        store.capture(hid(1), cap("hello")).await.expect("capture is discarded, not failed");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn open_keeps_capturing_when_only_the_index_cannot_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture");
        // Occupy the index's path with a directory so sqlite cannot open it as a file.
        std::fs::create_dir_all(index_path(&path)).expect("occupy index path");

        let store = OutputCapture::open(&path, DiskUsageLimit::Unlimited).await;
        assert_eq!(store.kind(), BackendKind::FjallUnindexed);
        store.capture(hid(1), cap("stored but unsearchable")).await.expect("capture");
        assert!(store.get(hid(1)).await.expect("get").is_some());
        assert!(store.searcher().search("unsearchable", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn nop_constructor_discards_everything() {
        let store = OutputCapture::nop();
        assert_eq!(store.kind(), BackendKind::Nop);
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
        let store = OutputCapture::nop();
        store.remove([hid(1), hid(2)]).await.expect("remove is discarded, not failed");
    }

    #[tokio::test]
    async fn searcher_sees_what_the_store_captures() {
        let (store, _dir) = temp_store().await;
        let searcher = store.searcher();
        store.capture(hid(1), cap("compilation error: missing semicolon")).await.expect("capture");

        let hits = searcher.search("semicolon", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));

        store.remove([hid(1)]).await.expect("remove");
        assert!(searcher.search("semicolon", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn search_on_a_nop_store_is_empty() {
        let store = OutputCapture::nop();
        store.capture(hid(1), cap("nothing is indexed here")).await.expect("capture");
        assert!(store.searcher().search("nothing", 10).await.expect("search").is_empty());
    }
}
