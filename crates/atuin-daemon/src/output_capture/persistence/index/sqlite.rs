use std::ffi::OsString;
use std::path::{Path, PathBuf};

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;

use super::schema::{Current, Schema, store};
use super::{Index, IndexError, OutputMatch};

/// A full-text search index backed by a sidecar sqlite FTS5 table.
#[derive(Debug)]
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
        let mut name = OsString::from(blob_dir.file_name().expect("the blob_dir cannot be root."));
        name.push(format!("-index-{}.sqlite", Current::VERSION));
        blob_dir.with_file_name(name)
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
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError> {
        Current::insert(&self.db, self.highlighter, id, text).await
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        Current::remove(&self.db, ids).await
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        Current::search(&self.db, self.highlighter, query, limit).await
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        Current::indexed_ids(&self.db).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::schema::CHUNK;
    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*uuid::Uuid::from_u128(n).as_bytes())
    }

    async fn search_hits(index: &SqliteIndex, query: &str, limit: usize) -> Vec<OutputMatch> {
        index.search(query, limit).await.try_collect().await.expect("search")
    }

    async fn temp_index() -> (SqliteIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = SqliteIndex::open(&dir.path().join("index.sqlite")).await.expect("open");
        (index, dir)
    }

    #[tokio::test]
    async fn open_stamps_the_schema_version() {
        let (index, _dir) = temp_index().await;
        let version: i64 =
            db::query_scalar("PRAGMA user_version").fetch_one(index.db.pool()).await.unwrap();
        assert_eq!(version, Current::VERSION);
    }

    #[tokio::test]
    async fn insert_then_search_finds_by_word() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "the build failed with an error").await.expect("insert");

        let hits = search_hits(&index, "error", 10).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
        let output = &hits[0].output;
        assert_eq!(output.display_plain().to_string(), "the build failed with an error");
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["error"]);
    }

    #[tokio::test]
    async fn every_match_in_the_output_gets_a_range() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "error one\nfine\nerror two").await.expect("insert");

        let hits = search_hits(&index, "error", 10).await;
        let output = &hits[0].output;
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["error", "error"]);
    }

    #[tokio::test]
    async fn marker_codepoints_in_the_text_cannot_forge_a_match() {
        let (index, _dir) = temp_index().await;
        // Plant the highlighter's own markers in the source; `insert` sanitizes them away, so they
        // cannot later be mistaken for a real highlight.
        let [open, close] = TextHighlighter::default().markers();
        let text = format!("{open}fake{close} real");
        index.insert(hid(1), &text).await.expect("insert");

        let hits = search_hits(&index, "real", 10).await;
        let output = &hits[0].output;
        assert_eq!(output.display_plain().to_string(), "fake real");
        let marked = output.as_ref();
        let got: Vec<&str> = output.ranges().map(|r| &marked[r]).collect();
        assert_eq!(got, vec!["real"]);
    }

    #[tokio::test]
    async fn search_returns_nothing_for_a_miss() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "hello world").await.expect("insert");
        assert!(search_hits(&index, "absent", 10).await.is_empty());
    }

    #[tokio::test]
    async fn blank_query_yields_no_results() {
        // A whitespace-only query has no searchable terms; search must short-circuit rather than
        // hand FTS5 an empty MATCH (which errors).
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "hello world").await.expect("insert");
        assert!(search_hits(&index, "   ", 10).await.is_empty());
    }

    #[tokio::test]
    async fn reinserting_the_same_id_does_not_duplicate() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "first text apple").await.expect("insert");
        index.insert(hid(1), "second text apple").await.expect("reinsert");

        let hits = search_hits(&index, "apple", 10).await;
        assert_eq!(hits.len(), 1, "the id appears once, not twice");
    }

    #[tokio::test]
    async fn remove_drops_the_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "removable content").await.expect("insert");
        index.remove(std::iter::once(hid(1))).await.expect("remove");
        assert!(search_hits(&index, "removable", 10).await.is_empty());
    }

    #[tokio::test]
    async fn indexed_ids_lists_every_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "one").await.expect("insert");
        index.insert(hid(2), "two").await.expect("insert");

        let mut ids: Vec<HistoryId> =
            index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, vec![hid(1), hid(2)]);
    }

    #[tokio::test]
    async fn indexed_ids_pages_past_the_chunk_boundary() {
        let (index, _dir) = temp_index().await;
        let count = CHUNK.get() as u128 + 17;
        for n in 1..=count {
            index.insert(hid(n), "body").await.expect("insert");
        }

        let ids: HashSet<HistoryId> =
            index.indexed_ids().await.try_collect().await.expect("indexed_ids");
        let expected: HashSet<HistoryId> = (1..=count).map(hid).collect();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn punctuation_in_the_query_does_not_error() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "a line with a path /usr/bin and a colon").await.expect("insert");
        // Bare FTS5 syntax chars would be a syntax error unquoted; sanitizing must swallow them.
        for q in ["\"unbalanced", "a OR", "path:", "(", "*", "-x"] {
            index
                .search(q, 10)
                .await
                .try_collect::<Vec<_>>()
                .await
                .unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
        }
    }

    #[tokio::test]
    async fn reopening_reuses_the_existing_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let index = SqliteIndex::open(&path).await.expect("open");
            index.insert(hid(1), "persistent data").await.expect("insert");
        }
        let index = SqliteIndex::open(&path).await.expect("reopen");
        // A matching schema version must not drop the table, so the row survives the reopen.
        let hits = search_hits(&index, "persistent", 10).await;
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn writes_wait_for_a_held_write_lock_on_a_cold_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let index = SqliteIndex::open(&path).await.expect("open");
            index.insert(hid(1), "held").await.expect("insert");
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
            async move { index.insert(hid(2), "queued").await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        db::query("COMMIT").execute(&mut *locker).await.expect("unlock");
        drop(locker);

        removing.await.expect("join").expect("remove waits for the lock");
        inserting.await.expect("join").expect("insert waits for the lock");
        assert!(search_hits(&index, "held", 10).await.is_empty());
        assert_eq!(search_hits(&index, "queued", 10).await.len(), 1);
    }
}
