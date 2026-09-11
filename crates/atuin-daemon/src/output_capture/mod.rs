mod backend;
mod gc;
mod index;
mod text;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_client::settings::DiskUsageLimit;
use backend::{AnyBackend, Backend as _, FjallBackend, NopBackend};
pub use backend::{BackendKind, CaptureError, DeleteOutputError, GetOutputError};
use gc::Gc;
use index::{NopIndex, SqliteIndex};
pub use index::{AnyIndex, Index, IndexError, OutputMatch};
use tokio::task::JoinHandle;
use tracing::{error, warn};

/// Failure to reconcile the derived search index against the store.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Backend(#[from] GetOutputError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// The store and its derived index, driven by the background tasks (the fjall flusher lives in the
/// backend; the [`Gc`] and reconcile task live on [`OutputCapture`]). It deliberately holds no task
/// handles, so a task can own a clone of it without a reference cycle back to the owner.
#[derive(Debug)]
struct Inner {
    /// The source of truth for captured output.
    backend: AnyBackend,
    /// A derived, rebuildable full-text index over that output.
    index: AnyIndex,
}

impl Inner {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        // The visible text is read before the value moves into the backend.
        let text = text::indexable_text(&capture);
        self.backend.capture(id, capture).await?;

        // Indexing is best-effort: a failure here must never sink the capture. Boot reconcile heals
        // any entry the index missed.
        if let Err(err) = self.index.insert(id, &text).await {
            warn!(?err, %id, "failed to index captured output; search may miss it until reconcile");
        }
        Ok(())
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.backend.get(id).await
    }

    async fn remove(&self, ids: Vec<HistoryId>) -> Result<(), DeleteOutputError> {
        let result = self.backend.remove(&ids).await;

        // Best-effort, like `capture`: the backend is authoritative, so its result is what we
        // return; a stale index entry left behind is dropped by the next reconcile.
        if let Err(err) = self.index.remove(&ids).await {
            warn!(?err, "failed to drop ids from the output search index");
        }
        result
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError> {
        self.index.search(query, limit).await
    }

    fn estimated_disk_space(&self) -> u64 {
        self.backend.estimated_disk_space()
    }

    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.backend.eviction_candidates(reclaim_bytes).await
    }

    /// Bring the derived index back in line with the store: drop index entries whose capture is
    /// gone, and index captures the index is missing (a rebuild is just this against an empty
    /// index). The index ids are read *before* the backend ids so a capture landing mid-reconcile
    /// is only ever seen as "missing" (and re-indexed), never mistaken for a stale entry to delete.
    async fn reconcile(&self) -> Result<(), ReconcileError> {
        let index_set: HashSet<HistoryId> = self.index.indexed_ids().await?.into_iter().collect();
        let backend_ids = self.backend.all_ids().await?;
        let backend_set: HashSet<HistoryId> = backend_ids.iter().copied().collect();

        let stale: Vec<HistoryId> = index_set.difference(&backend_set).copied().collect();
        if !stale.is_empty() {
            self.index.remove(&stale).await?;
        }

        for id in backend_ids {
            if index_set.contains(&id) {
                continue;
            }
            // A concurrent delete may have removed it since we listed ids; only index what's there.
            if let Some(capture) = self.backend.get(id).await? {
                self.index.insert(id, &text::indexable_text(&capture)).await?;
            }
        }
        Ok(())
    }
}

/// [`OutputCapture`] is the core engine responsible for collecting command output.
///
/// It owns the fjall backend (the source of truth) and a derived sqlite full-text index side by
/// side, keeping them consistent: every write and delete touches both, and a background reconcile
/// heals any drift. A read-only [`reader`](Self::reader) clone of the index is handed to the search
/// service to serve queries.
#[derive(Debug)]
pub struct OutputCapture {
    inner: Arc<Inner>,
    // Held only to abort their background tasks on drop; never read.
    _gc: Option<Gc>,
    reconcile_task: Option<JoinHandle<()>>,
}

impl OutputCapture {
    #[must_use]
    pub async fn open(path: impl AsRef<Path>, max_disk_usage: DiskUsageLimit) -> Self {
        let path = path.as_ref();

        let backend = match FjallBackend::open(path) {
            Ok(backend) => AnyBackend::Fjall(backend),
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
        let index = match SqliteIndex::open(&index_path(path)).await {
            Ok(index) => AnyIndex::Sqlite(index),
            Err(err) => {
                error!(
                    ?err,
                    ?path,
                    "failed to open the output search index; search over captured output is disabled"
                );
                AnyIndex::Nop(NopIndex)
            }
        };

        let inner = Arc::new(Inner { backend, index });

        // Reconcile in the background so boot isn't blocked by a large first-time index build.
        let reconcile_task = {
            let inner = inner.clone();
            tokio::spawn(async move {
                if let Err(err) = inner.reconcile().await {
                    warn!(?err, "failed to reconcile the output search index against the store");
                }
            })
        };

        // The gc drives `inner` from a background task, holding only a clone of it; `inner` points
        // at no task, so that clone forms no cycle that would keep the task alive.
        let gc = gc::resolve_budget(path, max_disk_usage).map(|budget| Gc::spawn(inner.clone(), budget));

        Self {
            inner,
            _gc: gc,
            reconcile_task: Some(reconcile_task),
        }
    }

    #[must_use]
    pub fn nop() -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: AnyBackend::Nop(NopBackend),
                index: AnyIndex::Nop(NopIndex),
            }),
            _gc: None,
            reconcile_task: None,
        }
    }

    /// A capture store whose every backend operation fails, standing in for a broken backend.
    ///
    /// Lets a test prove that a broken output store never sinks a primary operation (for example,
    /// that deleting history still succeeds when its captured output cannot be removed). Paired with
    /// a nop index so the failure under test is the backend's.
    #[cfg(test)]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: AnyBackend::Failing(backend::FailingBackend),
                index: AnyIndex::Nop(NopIndex),
            }),
            _gc: None,
            reconcile_task: None,
        }
    }

    #[must_use]
    pub fn kind(&self) -> BackendKind {
        BackendKind::from(&self.inner.backend)
    }

    /// Capture a command and associate it with the given history id.
    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        self.inner.capture(id, capture).await
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.inner.get(id).await
    }

    /// Forget the captured output of every history id in `ids`.
    ///
    /// Removing an absent id is a no-op, so this is safe to call for a batch that mixes captured and
    /// never-captured ids.
    pub async fn remove(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<(), DeleteOutputError> {
        self.inner.remove(ids.into_iter().collect()).await
    }

    /// Relevance-ranked full-text matches over captured output, most relevant first.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<OutputMatch>, IndexError> {
        self.inner.search(query, limit).await
    }

    /// A read-only handle to the search index, for the search service to serve queries against.
    #[must_use]
    pub fn reader(&self) -> AnyIndex {
        self.inner.index.clone()
    }
}

impl Drop for OutputCapture {
    fn drop(&mut self) {
        if let Some(task) = self.reconcile_task.take() {
            task.abort();
        }
    }
}

/// The sqlite index lives beside the fjall store directory as a sibling file, so it never lands
/// among fjall's own files. `.../output-capture` becomes `.../output-capture-index.sqlite`.
fn index_path(fjall_dir: &Path) -> PathBuf {
    let mut name = fjall_dir
        .file_name()
        .map_or_else(|| OsString::from("output-capture"), OsString::from);
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
        let store = OutputCapture::open(dir.path().join("capture"), DiskUsageLimit::Unlimited).await;
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
    async fn capture_then_search_finds_the_output() {
        let (store, _dir) = temp_store().await;
        store.capture(hid(1), cap("compilation error: missing semicolon")).await.expect("capture");

        let hits = store.search("semicolon", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn search_matches_the_visible_text_of_colorized_output() {
        let (store, _dir) = temp_store().await;
        // Red "fatal", reset, then plain text -- the escapes must not hide the word.
        store.capture(hid(1), cap("\x1b[31mfatal\x1b[0m: disk full")).await.expect("capture");

        let hits = store.search("fatal", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn remove_drops_the_output_from_search() {
        let (store, _dir) = temp_store().await;
        store.capture(hid(1), cap("searchable content")).await.expect("capture");
        store.remove([hid(1)]).await.expect("remove");
        assert!(store.search("searchable", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn search_on_a_nop_store_is_empty() {
        let store = OutputCapture::nop();
        store.capture(hid(1), cap("nothing is indexed here")).await.expect("capture");
        assert!(store.search("nothing", 10).await.expect("search").is_empty());
    }

    /// Build an `Inner` with a fresh backend and index over the same temp dir, for reconcile tests.
    async fn temp_inner(dir: &Path) -> Inner {
        let backend = FjallBackend::open(dir.join("store")).expect("open backend");
        let index = SqliteIndex::open(&dir.join("index.sqlite")).await.expect("open index");
        Inner {
            backend: AnyBackend::Fjall(backend),
            index: AnyIndex::Sqlite(index),
        }
    }

    #[tokio::test]
    async fn reconcile_indexes_captures_missing_from_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inner = temp_inner(dir.path()).await;
        // Write straight to the backend so the index never sees it -- as if the index write was lost.
        inner.backend.capture(hid(1), cap("orphaned output text")).await.expect("capture");

        assert!(inner.search("orphaned", 10).await.expect("search").is_empty(), "not yet indexed");
        inner.reconcile().await.expect("reconcile");

        let hits = inner.search("orphaned", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn reconcile_drops_index_entries_without_a_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inner = temp_inner(dir.path()).await;
        // An index entry whose capture never existed in the backend (drift from a crashed delete).
        inner.index.insert(hid(9), "ghost entry").await.expect("insert");

        inner.reconcile().await.expect("reconcile");
        assert!(inner.search("ghost", 10).await.expect("search").is_empty());
    }
}
