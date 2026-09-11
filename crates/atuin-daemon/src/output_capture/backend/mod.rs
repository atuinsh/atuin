//! The output capture backend: a [`Storage`] holding the captures, coupled with the [`Index`] that
//! searches them.

mod gc;
mod index;
mod storage;

use std::collections::HashSet;

use atuin_client::history::{CommandCapture, HistoryId};
use enum_dispatch::enum_dispatch;
pub use gc::Gc;
pub use index::{Index, IndexError, NopIndex, OutputMatch, SqliteIndex};
#[cfg(test)]
pub use storage::FailingStorage;
pub use storage::{
    CaptureError, DeleteOutputError, FjallStorage, GetOutputError, NopStorage, Storage,
};
use tracing::warn;

/// Failure to reconcile the derived search index against the storage.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Storage(#[from] GetOutputError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// A [`Storage`] and a derived [`Index`] over it.
#[derive(Debug)]
pub struct Backend<S, I> {
    /// The source of truth for captured output.
    storage: S,
    /// A rebuildable full-text index over that output.
    index: I,
}

impl<S: Storage, I: Index> Backend<S, I> {
    pub fn new(storage: S, index: I) -> Self {
        Self { storage, index }
    }
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait OutputBackend {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError>;

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError>;

    async fn remove(&self, ids: &[HistoryId]) -> Result<(), DeleteOutputError>;

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError>;

    fn estimated_disk_space(&self) -> u64;

    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError>;

    async fn reconcile(&self) -> Result<(), ReconcileError>;
}

impl<S: Storage, I: Index> OutputBackend for Backend<S, I> {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        let text = capture.plaintext();
        self.storage.capture(id, capture).await?;

        // Indexing here is best-effort. If we fail, we fail, it's sad but hopefully the next
        // reconcile will pick it up.
        if let Err(err) = self.index.insert(id, &text).await {
            warn!(?err, %id, "failed to index captured output; search may miss it until reconcile");
        }
        Ok(())
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.storage.get(id).await
    }

    async fn remove(&self, ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        let result = self.storage.remove(ids.iter().copied()).await;

        if let Err(err) = self.index.remove(ids.iter().copied()).await {
            warn!(?err, "failed to drop ids from the output search index");
        }

        result
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError> {
        self.index.search(query, limit).await
    }

    fn estimated_disk_space(&self) -> u64 {
        self.storage.estimated_disk_space()
    }

    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.storage.eviction_candidates(reclaim_bytes).await
    }

    async fn reconcile(&self) -> Result<(), ReconcileError> {
        let index_set: HashSet<HistoryId> = self.index.indexed_ids().await?.into_iter().collect();
        let storage_ids = self.storage.all_ids().await?;
        let storage_set: HashSet<HistoryId> = storage_ids.iter().copied().collect();

        let stale: Vec<HistoryId> = index_set.difference(&storage_set).copied().collect();
        if !stale.is_empty() {
            self.index.remove(stale.iter().copied()).await?;
        }

        for id in storage_ids {
            if index_set.contains(&id) {
                continue;
            }
            // A concurrent delete may have removed it since we listed ids; only index what's there.
            if let Some(capture) = self.storage.get(id).await? {
                self.index.insert(id, &capture.plaintext()).await?;
            }
        }
        Ok(())
    }
}

/// The fjall store with its sqlite full-text index beside it.
pub type FjallBackend = Backend<FjallStorage, SqliteIndex>;
/// The fjall store alone: captures persist, but the index failed to open so search finds nothing.
pub type FjallUnindexedBackend = Backend<FjallStorage, NopIndex>;
/// Output capture is disabled: everything is discarded and nothing is found.
pub type NopBackend = Backend<NopStorage, NopIndex>;
/// A backend whose every storage operation fails; see [`FailingStorage`].
#[cfg(test)]
pub type FailingBackend = Backend<FailingStorage, NopIndex>;

/// Every pairing of storage and index the daemon can run on.
#[derive(Debug, strum_macros::EnumDiscriminants)]
#[strum_discriminants(name(BackendKind))]
#[enum_dispatch(OutputBackend)]
pub enum AnyBackend {
    Fjall(FjallBackend),
    FjallUnindexed(FjallUnindexedBackend),
    Nop(NopBackend),
    /// Built only by the [`OutputCapture::failing`](crate::OutputCapture::failing) test hook.
    /// Never selected in production.
    #[cfg(test)]
    Failing(FailingBackend),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

    async fn temp_backend(dir: &Path) -> FjallBackend {
        let storage = FjallStorage::open(dir.join("store")).expect("open storage");
        let index = SqliteIndex::open(&dir.join("index.sqlite")).await.expect("open index");
        Backend::new(storage, index)
    }

    #[tokio::test]
    async fn capture_then_search_finds_the_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend
            .capture(hid(1), cap("compilation error: missing semicolon"))
            .await
            .expect("capture");

        let hits = backend.search("semicolon", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn search_matches_the_visible_text_of_colorized_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        // Red "fatal", reset, then plain text -- the escapes must not hide the word.
        backend.capture(hid(1), cap("\x1b[31mfatal\x1b[0m: disk full")).await.expect("capture");

        let hits = backend.search("fatal", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        let output = &hits[0].output;
        assert_eq!(output.display_plain().to_string(), "fatal: disk full");
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["fatal"]);
    }

    #[tokio::test]
    async fn remove_drops_the_output_from_search() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.capture(hid(1), cap("searchable content")).await.expect("capture");
        backend.remove(&[hid(1)]).await.expect("remove");
        assert!(backend.search("searchable", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn reconcile_indexes_captures_missing_from_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        // Write straight to the storage so the index never sees it -- as if the index write was lost.
        backend.storage.capture(hid(1), cap("orphaned output text")).await.expect("capture");

        assert!(
            backend.search("orphaned", 10).await.expect("search").is_empty(),
            "not yet indexed"
        );
        backend.reconcile().await.expect("reconcile");

        let hits = backend.search("orphaned", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn reconcile_drops_index_entries_without_a_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        // An index entry whose capture never existed in the storage (drift from a crashed delete).
        backend.index.insert(hid(9), "ghost entry").await.expect("insert");

        backend.reconcile().await.expect("reconcile");
        assert!(backend.search("ghost", 10).await.expect("search").is_empty());
    }
}
