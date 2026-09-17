mod blob;
mod index;
mod snippet;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::futures::stream::{ChunkedStream, EitherOrBoth, try_merge_join};
#[cfg(test)]
pub use blob::FailingBlobStore;
pub use blob::{
    AnyBlobStore, BlobStore, CaptureError, DeleteOutputError, FjallBlobStore, GetOutputError,
    NopBlobStore,
};
use futures::{StreamExt, TryStreamExt, stream};
#[cfg(test)]
pub use index::FailingIndex;
pub use index::{AnyIndex, Index, IndexError, NopIndex, RankedMatch, SqliteIndex};
pub use snippet::{OutputLine, OutputMatch};
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error(transparent)]
    BlobStore(#[from] GetOutputError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// Bodies highlighted per sqlite round trip.
const HIGHLIGHT_BATCH: usize = 64;

#[derive(Debug)]
pub struct OutputStore {
    blob: AnyBlobStore,
    index: AnyIndex,
}

impl OutputStore {
    pub fn new(blob: AnyBlobStore, index: AnyIndex) -> Self {
        Self { blob, index }
    }

    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        let text = capture.plaintext();
        self.blob.capture(id, capture).await?;

        if let Err(err) = self.index.insert(id, &text).await {
            warn!(?err, %id, "failed to index captured output; search may miss it until reconcile");
        }
        Ok(())
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.blob.get(id).await
    }

    pub async fn remove(&self, ids: &[HistoryId]) -> Result<(), DeleteOutputError> {
        let result = self.blob.remove(ids.iter().copied()).await;

        if let Err(err) = self.index.remove(ids.iter().copied()).await {
            warn!(?err, "failed to drop ids from the output search index");
        }

        result
    }

    /// Relevance-ranked hits, each reduced to the lines within `context` of a match.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        context: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        // Only the ranking (history_id + score) comes from the contentless index; it is small, so
        // collecting it keeps error handling simple. The bodies -- each up to `max_output_size` --
        // are the memory risk, so they are hydrated lazily from the blob as `highlight` pulls its
        // batches, and the caller dropping the stream stops that work early.
        let ranked: Vec<RankedMatch> =
            match self.index.search(query, limit).await.try_collect().await {
                Ok(ranked) => ranked,
                Err(err) => return ChunkedStream::from_error(err),
            };

        // The index is derived and can briefly hold entries whose blob was deleted -- a
        // capture/remove race, a swallowed index write, or reconcile lag. The blob is
        // authoritative: a hit is highlighted from its stored capture, and one whose capture is
        // gone is dropped. (This can yield fewer than `limit` hits even when more live matches
        // exist further down the ranking.)
        let blob = self.blob.clone();
        let hits = stream::iter(ranked).filter_map(move |hit| {
            let blob = blob.clone();
            async move {
                match blob.get(hit.history_id).await {
                    Ok(Some(capture)) => Some((hit, capture)),
                    Ok(None) => None, // the capture is gone; drop the stale index hit
                    // A transient read failure hides only this hit, not the whole search.
                    Err(err) => {
                        warn!(
                            ?err,
                            id = %hit.history_id,
                            "failed to read a search hit's capture; dropping it",
                        );
                        None
                    }
                }
            }
        });

        // Highlighting goes through sqlite in batches; each batch is one round trip.
        let index = self.index.clone();
        let query = query.to_owned();
        ChunkedStream::new(hits.chunks(HIGHLIGHT_BATCH).then(move |batch| {
            let index = index.clone();
            let query = query.clone();
            async move {
                let bodies = batch.iter().map(|(_, capture)| capture.plaintext()).collect();
                let highlighted = match index.highlight(&query, bodies).await {
                    Ok(highlighted) => highlighted,
                    Err(err) => return vec![Err(err)],
                };
                batch
                    .into_iter()
                    .zip(highlighted)
                    .map(|((hit, capture), body)| {
                        // `plaintext` joins the kept head and tail with one newline, so the tail
                        // starts right after the head's last line.
                        let tail_from = capture
                            .output_end
                            .as_ref()
                            .map(|_| capture.output_start.lines().count());
                        Ok(OutputMatch {
                            history_id: hit.history_id,
                            lines: snippet::snippet(&body, tail_from, context),
                            score: hit.score,
                        })
                    })
                    .collect()
            }
        }))
    }

    pub fn estimated_disk_space(&self) -> u64 {
        self.blob.estimated_disk_space()
    }

    pub async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.blob.eviction_candidates(reclaim_bytes).await
    }

    pub async fn reconcile(&self) -> Result<(), ReconcileError> {
        let blob = self.blob.all_ids().await.items().filter_map(|res| async move {
            match res {
                Ok(id) => Some(Ok::<HistoryId, ReconcileError>(id)),
                Err(err) => {
                    warn!(?err, "skipping an unreadable id from the output store during reconcile");
                    None
                }
            }
        });

        let index = self.index.indexed_ids().await.items().filter_map(|res| async move {
            match res {
                Ok(id) => Some(Ok::<HistoryId, ReconcileError>(id)),
                Err(err) => {
                    warn!(?err, "skipping an unreadable id from the search index during reconcile");
                    None
                }
            }
        });

        try_merge_join(blob, index)
            .try_for_each(|side| async move {
                match side {
                    EitherOrBoth::Left(id) => match self.blob.get(id).await {
                        Ok(Some(capture)) => {
                            self.index.insert(id, &capture.plaintext()).await?;
                        }
                        Ok(None) => {}
                        // One unreadable capture must not stall reconciliation of everything
                        // sorted after it; index failures still abort, since those are systemic.
                        Err(err) => {
                            warn!(?err, %id, "skipping an unreadable capture during reconcile");
                        }
                    },
                    EitherOrBoth::Right(id) => {
                        if !self.blob.contains(id).await? {
                            self.index.remove(std::iter::once(id)).await?;
                        }
                    }
                    EitherOrBoth::Both(..) => {}
                }
                Ok(())
            })
            .await
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
        let blob = FjallBlobStore::open(dir.join("store")).expect("open blob");
        let index = SqliteIndex::open(&dir.join("index.sqlite")).await.expect("open index");
        OutputStore::new(AnyBlobStore::Fjall(blob), AnyIndex::Sqlite(index))
    }

    async fn search_hits(store: &OutputStore, query: &str, limit: usize) -> Vec<OutputMatch> {
        store.search(query, limit, 0).await.try_collect().await.expect("search")
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
        assert_eq!(hits[0].lines.len(), 1);
        let plain = hits[0].lines[0].content.to_plain();
        assert_eq!(plain.text, "fatal: disk full");
        assert_eq!(plain.ranges, vec![0..5]);
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
    async fn search_hides_index_entries_whose_capture_is_gone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;

        // An index entry with no backing capture -- the drift a capture/remove race or a swallowed
        // index write can leave behind. The blob is authoritative, so search must not surface it.
        backend.index.insert(hid(9), "ghost output text").await.expect("insert");
        assert!(search_hits(&backend, "ghost", 10).await.is_empty());

        // A normally-captured entry is still found, so the guard only hides the orphan.
        backend.capture(hid(1), cap("real ghost output")).await.expect("capture");
        let hits = search_hits(&backend, "ghost", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn capture_and_remove_survive_an_erroring_index() {
        // The index is derived, so a live index failure must not fail the capture or the removal;
        // the blob store stays authoritative and reconcile heals the index later.
        let dir = tempfile::tempdir().expect("tempdir");
        let blob = FjallBlobStore::open(dir.path().join("store")).expect("open blob");
        let store = OutputStore::new(AnyBlobStore::Fjall(blob), AnyIndex::Failing(FailingIndex));

        store.capture(hid(1), cap("still stored")).await.expect("capture survives index failure");
        assert_eq!(
            store.get(hid(1)).await.expect("get").expect("present").output_start,
            "still stored"
        );

        store.remove(&[hid(1)]).await.expect("remove survives index failure");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn reconcile_indexes_captures_missing_from_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.blob.capture(hid(1), cap("orphaned output text")).await.expect("capture");

        assert!(search_hits(&backend, "orphaned", 10).await.is_empty(), "not yet indexed");
        backend.reconcile().await.expect("reconcile");

        let hits = search_hits(&backend, "orphaned", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
    }

    #[tokio::test]
    async fn reconcile_skips_an_unreadable_capture_and_indexes_the_rest() {
        use std::collections::HashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        let AnyBlobStore::Fjall(blob) = &backend.blob else {
            unreachable!()
        };
        blob.capture(hid(1), cap("stored 1")).await.expect("capture");
        blob.corrupt(hid(2));
        blob.capture(hid(3), cap("stored 3")).await.expect("capture");

        backend.reconcile().await.expect("reconcile");

        let indexed: HashSet<HistoryId> =
            backend.index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        assert_eq!(indexed, [hid(1), hid(3)].into_iter().collect());
    }

    #[tokio::test]
    async fn reconcile_drops_index_entries_without_a_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;
        backend.index.insert(hid(9), "ghost entry").await.expect("insert");

        backend.reconcile().await.expect("reconcile");
        assert!(search_hits(&backend, "ghost", 10).await.is_empty());
    }

    #[tokio::test]
    async fn reconcile_syncs_a_mixed_index_in_one_pass() {
        use std::collections::HashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let backend = temp_backend(dir.path()).await;

        for n in [1u128, 2, 4] {
            backend.blob.capture(hid(n), cap(&format!("stored {n}"))).await.expect("capture");
        }
        backend.index.insert(hid(2), "stored 2").await.expect("insert");
        backend.index.insert(hid(3), "ghost 3").await.expect("insert");

        backend.reconcile().await.expect("reconcile");

        let indexed: HashSet<HistoryId> =
            backend.index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        assert_eq!(indexed, [hid(1), hid(2), hid(4)].into_iter().collect());
    }
}
