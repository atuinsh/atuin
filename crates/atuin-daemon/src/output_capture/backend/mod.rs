mod index;
mod storage;

use std::collections::HashSet;
use std::pin::pin;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::futures::stream::ChunkedStream;
use futures::TryStreamExt;
pub use index::{AnyIndex, Index, IndexError, NopIndex, SqliteIndex};
#[cfg(test)]
pub use storage::FailingStorage;
pub use storage::{
    AnyStorage, CaptureError, DeleteOutputError, FjallStorage, GetOutputError, NopStorage, Storage,
};
use tracing::warn;

use super::OutputMatch;

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Storage(#[from] GetOutputError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

#[derive(Debug)]
pub struct OutputStore {
    storage: AnyStorage,
    index: AnyIndex,
}

impl OutputStore {
    pub fn new(storage: AnyStorage, index: AnyIndex) -> Self {
        Self { storage, index }
    }

    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        let text = capture.plaintext();
        self.storage.capture(id, capture).await?;

        if let Err(err) = self.index.insert(id, &text).await {
            warn!(?err, %id, "failed to index captured output; search may miss it until reconcile");
        }
        Ok(())
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.storage.get(id).await
    }

    pub async fn remove(&self, ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        let result = self.storage.remove(ids.iter().copied()).await;

        if let Err(err) = self.index.remove(ids.iter().copied()).await {
            warn!(?err, "failed to drop ids from the output search index");
        }

        result
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        self.index.search(query, limit).await
    }

    pub fn estimated_disk_space(&self) -> u64 {
        self.storage.estimated_disk_space()
    }

    pub async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.storage.eviction_candidates(reclaim_bytes).await
    }

    pub async fn reconcile(&self) -> Result<(), ReconcileError> {
        let mut unseen: HashSet<HistoryId> = self.index.indexed_ids().await.try_collect().await?;

        let mut storage_ids = pin!(self.storage.all_ids().await.items());
        while let Some(id) = storage_ids.try_next().await? {
            if unseen.remove(&id) {
                continue;
            }
            if let Some(capture) = self.storage.get(id).await? {
                self.index.insert(id, &capture.plaintext()).await?;
            }
        }

        if !unseen.is_empty() {
            self.index.remove(unseen.into_iter()).await?;
        }
        Ok(())
    }
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

    async fn temp_backend(dir: &Path) -> OutputStore {
        let storage = FjallStorage::open(dir.join("store")).expect("open storage");
        let index = SqliteIndex::open(&dir.join("index.sqlite")).await.expect("open index");
        OutputStore::new(AnyStorage::Fjall(storage), AnyIndex::Sqlite(index))
    }

    async fn search_hits(store: &OutputStore, query: &str, limit: usize) -> Vec<OutputMatch> {
        store.search(query, limit).await.try_collect().await.expect("search")
    }

    #[tokio::test]
    async fn capture_then_search_finds_the_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend
            .capture(hid(1), cap("compilation error: missing semicolon"))
            .await
            .expect("capture");

        let hits = search_hits(&backend, "semicolon", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn search_matches_the_visible_text_of_colorized_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.capture(hid(1), cap("\x1b[31mfatal\x1b[0m: disk full")).await.expect("capture");

        let hits = search_hits(&backend, "fatal", 10).await;
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
        assert!(search_hits(&backend, "searchable", 10).await.is_empty());
    }

    #[tokio::test]
    async fn reconcile_indexes_captures_missing_from_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.storage.capture(hid(1), cap("orphaned output text")).await.expect("capture");

        assert!(search_hits(&backend, "orphaned", 10).await.is_empty(), "not yet indexed");
        backend.reconcile().await.expect("reconcile");

        let hits = search_hits(&backend, "orphaned", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn reconcile_drops_index_entries_without_a_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.index.insert(hid(9), "ghost entry").await.expect("insert");

        backend.reconcile().await.expect("reconcile");
        assert!(search_hits(&backend, "ghost", 10).await.is_empty());
    }
}
