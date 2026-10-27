use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;
use tracing::warn;

use super::schema::{Current, Schema, store};
use super::{HighlightedCapture, Index, IndexError, RankedMatch};

/// A full-text search index backed by a sidecar sqlite FTS5 table.
#[derive(Debug, Clone)]
pub struct SqliteIndex {
    db: Sqlite,
    /// A type which allows type-safe operation on SQL `highlight` statements.
    highlighter: TextHighlighter,
}

impl SqliteIndex {
    /// Open (creating if missing) the index at `path`, running any pending migration.
    pub async fn open(path: &Path) -> Result<Self, IndexError> {
        let db = Sqlite::builder(path.as_os_str())
            .restrict_permissions()
            .open()
            .await
            .map_err(|err| IndexError::Storage(Box::new(err)))?;

        let index = Self {
            db,
            highlighter: TextHighlighter::default(),
        };

        index.migrate().await?;
        Ok(index)
    }

    /// Create a new path that's appropriate for the [`SqliteIndex`] relative to the `BlobStore`'s
    /// path.
    ///
    /// # Panics
    ///
    /// If the given directory is the root directory.
    pub fn path(blob_dir: &Path) -> PathBuf {
        Self::versioned_path(blob_dir, Current::VERSION)
    }

    fn versioned_path(blob_dir: &Path, version: i64) -> PathBuf {
        let mut name = OsString::from(blob_dir.file_name().expect("the blob_dir cannot be root."));
        name.push(format!("-index-{version}.sqlite"));
        blob_dir.with_file_name(name)
    }

    /// Delete the indexes that older schema versions left beside `blob_dir`. Each version has its
    /// own file (see [`Self::path`]), so nothing opens an older one again.
    pub async fn remove_stale(blob_dir: &Path) {
        for version in 1..Current::VERSION {
            // sqlite in WAL mode keeps the `-wal` and `-shm` files beside the database.
            for suffix in ["", "-wal", "-shm"] {
                let mut file = Self::versioned_path(blob_dir, version).into_os_string();
                file.push(suffix);
                match tokio::fs::remove_file(&file).await {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => warn!(?err, ?file, "failed to remove a stale output search index"),
                }
            }
        }
    }

    async fn migrate(&self) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let version: i64 =
            db::query_scalar("PRAGMA user_version").fetch_one(pool).await.map_err(store)?;
        if version == Current::VERSION {
            return Ok(());
        }

        Current::setup(&self.db).await?;
        db::query(sqlx::AssertSqlSafe(format!("PRAGMA user_version = {}", Current::VERSION)))
            .execute(pool)
            .await
            .map_err(store)?;
        Ok(())
    }
}

impl Index for SqliteIndex {
    async fn insert(
        &self,
        id: HistoryId,
        head: &str,
        tail: Option<&str>,
    ) -> Result<(), IndexError> {
        Current::insert(&self.db, self.highlighter, id, head, tail).await
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        Current::remove(&self.db, ids).await
    }

    async fn search(
        &self,
        terms: impl IntoIterator<Item = &str>,
        limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        Current::search(&self.db, terms, limit).await
    }

    async fn highlight(
        &self,
        terms: impl IntoIterator<Item = &str>,
        head: &str,
        tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError> {
        Current::highlight(&self.db, self.highlighter, terms, head, tail).await
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        Current::indexed_ids(&self.db).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use atuin_common::string::highlighted::HighlightedString;
    use rstest::rstest;

    use super::super::schema::CHUNK;
    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*uuid::Uuid::from_u128(n).as_bytes())
    }

    /// Every ranked hit for `terms`, each highlighted over `body`.
    async fn search_hits(
        index: &SqliteIndex,
        terms: impl IntoIterator<Item = &str> + Clone,
        body: &str,
    ) -> Vec<(HistoryId, HighlightedString)> {
        let ranked: Vec<RankedMatch> =
            index.search(terms.clone(), 10).await.try_collect().await.expect("search");
        let mut hits = Vec::new();
        for hit in ranked {
            let highlighted =
                index.highlight(terms.clone(), body, None).await.expect("highlight").head;
            hits.push((hit.history_id, highlighted));
        }
        hits
    }

    async fn temp_index() -> (SqliteIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = SqliteIndex::open(&dir.path().join("index.sqlite")).await.expect("open");
        (index, dir)
    }

    #[rstest]
    #[tokio::test]
    async fn open_stamps_the_schema_version() {
        let (index, _dir) = temp_index().await;
        let version: i64 =
            db::query_scalar("PRAGMA user_version").fetch_one(index.db.pool()).await.unwrap();
        assert_eq!(version, Current::VERSION);
    }

    #[rstest]
    #[tokio::test]
    async fn remove_stale_deletes_older_versions_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blob_dir = dir.path().join("output");
        let stale = SqliteIndex::versioned_path(&blob_dir, Current::VERSION - 1);
        let stale_files = ["", "-wal", "-shm"].map(|suffix| {
            let mut file = stale.clone().into_os_string();
            file.push(suffix);
            PathBuf::from(file)
        });
        for file in &stale_files {
            std::fs::write(file, b"stale").expect("write a stale file");
        }
        let current = SqliteIndex::open(&SqliteIndex::path(&blob_dir)).await.expect("open");
        current.insert(hid(1), "kept", None).await.expect("insert");

        SqliteIndex::remove_stale(&blob_dir).await;

        for file in &stale_files {
            assert!(!file.exists(), "{} survived", file.display());
        }
        assert_eq!(search_hits(&current, ["kept"], "kept").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn insert_then_search_finds_by_word() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "the build failed with an error", None).await.expect("insert");

        let hits = search_hits(&index, ["error"], "the build failed with an error").await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, hid(1));
        let output = &hits[0].1;
        assert_eq!(output.display_plain().to_string(), "the build failed with an error");
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["error"]);
    }

    #[rstest]
    #[tokio::test]
    async fn every_match_in_the_output_gets_a_range() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "error one\nfine\nerror two", None).await.expect("insert");

        let hits = search_hits(&index, ["error"], "error one\nfine\nerror two").await;
        let output = &hits[0].1;
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["error", "error"]);
    }

    #[rstest]
    #[tokio::test]
    async fn marker_codepoints_in_the_text_cannot_forge_a_match() {
        let (index, _dir) = temp_index().await;
        // Plant the highlighter's own markers in the source; `insert` sanitizes them away, so they
        // cannot later be mistaken for a real highlight.
        let [open, close] = TextHighlighter::default().markers();
        let text = format!("{open}fake{close} real");
        index.insert(hid(1), &text, None).await.expect("insert");

        let hits = search_hits(&index, ["real"], &text).await;
        let output = &hits[0].1;
        assert_eq!(output.display_plain().to_string(), "fake real");
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["real"]);
    }

    #[rstest]
    #[tokio::test]
    async fn the_index_stores_no_body() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "kept only in the blob store", None).await.expect("insert");

        let content_tables: i64 =
            db::query_scalar("SELECT count(*) FROM sqlite_master WHERE name LIKE '%_content'")
                .fetch_one(index.db.pool())
                .await
                .unwrap();
        assert_eq!(content_tables, 0);
    }

    #[rstest]
    #[tokio::test]
    async fn a_body_that_drifted_from_the_index_is_returned_unhighlighted() {
        let (index, _dir) = temp_index().await;
        let hit = index
            .highlight(["error"], "re-rendered without the term", None)
            .await
            .expect("highlight")
            .head;
        assert_eq!(hit.display_plain().to_string(), "re-rendered without the term");
        assert_eq!(hit.ranges().count(), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn repeated_highlighting_does_not_leak_the_previous_body() {
        // The scratch table is per connection and reused; a stale row must never be returned.
        let (index, _dir) = temp_index().await;
        let first =
            index.highlight(["error"], "an error here", None).await.expect("highlight").head;
        assert_eq!(first.ranges().count(), 1);
        let second =
            index.highlight(["error"], "nothing to see", None).await.expect("highlight").head;
        assert_eq!(second.display_plain().to_string(), "nothing to see");
        assert_eq!(second.ranges().count(), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn search_returns_nothing_for_a_miss() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "hello world", None).await.expect("insert");
        assert!(search_hits(&index, ["absent"], "hello world").await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn limit_zero_is_unbounded_and_a_positive_limit_caps() {
        let (index, _dir) = temp_index().await;
        for i in 1..=5 {
            index.insert(hid(i), "error", None).await.expect("insert");
        }
        // 0 pages until the index is exhausted; a positive limit stops early.
        let unbounded: Vec<RankedMatch> =
            index.search(["error"], 0).await.try_collect().await.expect("search");
        assert_eq!(unbounded.len(), 5);
        let capped: Vec<RankedMatch> =
            index.search(["error"], 2).await.try_collect().await.expect("search");
        assert_eq!(capped.len(), 2);
    }

    #[rstest]
    #[tokio::test]
    async fn blank_query_yields_no_results() {
        // A whitespace-only query has no searchable terms; search must short-circuit rather than
        // hand FTS5 an empty MATCH (which errors).
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "hello world", None).await.expect("insert");
        assert!(search_hits(&index, ["   "], "hello world").await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn reinserting_the_same_id_does_not_duplicate() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "first text apple", None).await.expect("insert");
        index.insert(hid(1), "second text apple", None).await.expect("reinsert");

        let hits = search_hits(&index, ["apple"], "second text apple").await;
        assert_eq!(hits.len(), 1, "the id appears once, not twice");
    }

    #[rstest]
    #[tokio::test]
    async fn remove_drops_the_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "removable content", None).await.expect("insert");
        index.remove(std::iter::once(hid(1))).await.expect("remove");
        assert!(search_hits(&index, ["removable"], "removable content").await.is_empty());
    }

    /// A bulk removal mixing indexed and never-indexed ids drops exactly the indexed ones, from
    /// both the id table and the FTS index, however the ids split into statements.
    #[rstest]
    #[case::one_id_per_statement(1)]
    #[case::chunks_split_mid_batch(2)]
    #[case::one_chunk(1000)]
    #[tokio::test]
    async fn bulk_remove_drops_only_the_given_ids_across_chunks(#[case] keys_per_delete: usize) {
        let (index, _dir) = temp_index().await;
        for n in 1..=5 {
            index.insert(hid(n), &format!("word{n}"), None).await.expect("insert");
        }

        // Three indexed ids and two that were never indexed, interleaved across the chunks.
        let keys: Vec<[u8; 16]> =
            [1, 100, 3, 101, 5].into_iter().map(|n| hid(n).into_bytes()).collect();
        Current::remove_chunked(&index.db, &keys, keys_per_delete).await.expect("remove");

        let mut ids: Vec<HistoryId> =
            index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, vec![hid(2), hid(4)]);
        for n in [1, 3, 5] {
            let word = format!("word{n}");
            assert!(
                search_hits(&index, [word.as_str()], &word).await.is_empty(),
                "{word} still indexed"
            );
        }
        for n in [2, 4] {
            let word = format!("word{n}");
            assert_eq!(
                search_hits(&index, [word.as_str()], &word).await.len(),
                1,
                "{word} was removed"
            );
        }
    }

    #[rstest]
    #[tokio::test]
    async fn indexed_ids_lists_every_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "one", None).await.expect("insert");
        index.insert(hid(2), "two", None).await.expect("insert");

        let mut ids: Vec<HistoryId> =
            index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, vec![hid(1), hid(2)]);
    }

    #[rstest]
    #[tokio::test]
    async fn indexed_ids_pages_past_the_chunk_boundary() {
        let (index, _dir) = temp_index().await;
        let count = CHUNK.get() as u128 + 17;
        for n in 1..=count {
            index.insert(hid(n), "body", None).await.expect("insert");
        }

        let ids: HashSet<HistoryId> =
            index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        let expected: HashSet<HistoryId> = (1..=count).map(hid).collect();
        assert_eq!(ids, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn punctuation_in_the_query_does_not_error() {
        let (index, _dir) = temp_index().await;
        index
            .insert(hid(1), "a line with a path /usr/bin and a colon", None)
            .await
            .expect("insert");
        // Bare FTS5 syntax chars would be a syntax error unquoted; sanitizing must swallow them.
        for q in ["\"unbalanced", "a OR", "path:", "(", "*", "-x"] {
            index
                .search([q], 10)
                .await
                .try_collect::<Vec<_>>()
                .await
                .unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
        }
    }

    #[rstest]
    #[tokio::test]
    async fn reopening_reuses_the_existing_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let index = SqliteIndex::open(&path).await.expect("open");
            index.insert(hid(1), "persistent data", None).await.expect("insert");
        }
        let index = SqliteIndex::open(&path).await.expect("reopen");
        // A matching schema version must not drop the table, so the row survives the reopen.
        let hits = search_hits(&index, ["persistent"], "persistent data").await;
        assert_eq!(hits.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn writes_wait_for_a_held_write_lock_on_a_cold_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let index = SqliteIndex::open(&path).await.expect("open");
            index.insert(hid(1), "held", None).await.expect("insert");
        }
        // Reopening only reads `user_version`, so no pool connection has touched the FTS table
        // yet. FTS5 connects the virtual table when a connection first prepares a statement
        // against it, and inside a deferred transaction that read pins a snapshot which makes the
        // following write fail with SQLITE_BUSY at once instead of waiting on the busy handler.
        let index = SqliteIndex::open(&path).await.expect("reopen");

        let mut locker = index.db.pool().acquire().await.expect("acquire");
        db::query("BEGIN IMMEDIATE").execute(&mut *locker).await.expect("lock");

        let index = std::sync::Arc::new(index);
        let removing = tokio::spawn({
            let index = index.clone();
            async move { index.remove(std::iter::once(hid(1))).await }
        });
        let inserting = tokio::spawn({
            let index = index.clone();
            async move { index.insert(hid(2), "queued", None).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        db::query("COMMIT").execute(&mut *locker).await.expect("unlock");
        drop(locker);

        removing.await.expect("join").expect("remove waits for the lock");
        inserting.await.expect("join").expect("insert waits for the lock");
        assert!(search_hits(&index, ["held"], "held").await.is_empty());
        assert_eq!(search_hits(&index, ["queued"], "queued").await.len(), 1);
    }
}
