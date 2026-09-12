use std::num::NonZeroUsize;
use std::path::Path;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::{TextHighlighter, TextHighlighterBindExt};
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;
use sqlx::Row;

use super::{Index, IndexError, OutputMatch};

const CHUNK: NonZeroUsize = NonZeroUsize::new(512).unwrap();

/// Bump this whenever the on-disk index shape changes. On open a mismatch drops the table; the
/// fjall store is the source of truth, so the next reconcile repopulates it.
///
/// v2: `history_id` stored as a 16-byte BLOB rather than a 32-char hex string.
const SCHEMA_VERSION: i64 = 2;

/// A full-text search index backed by a sidecar sqlite FTS5 table.
#[derive(Debug)]
pub struct SqliteIndex {
    db: Sqlite,
    /// A type which allows type-safe operation on SQL `highlight` statements.
    highlighter: TextHighlighter,
}

fn store(err: sqlx::Error) -> IndexError {
    IndexError::Storage(Box::new(err))
}

fn id_from_bytes(raw: &[u8]) -> Result<HistoryId, IndexError> {
    let bytes: [u8; 16] = raw.try_into().map_err(|err| IndexError::Storage(Box::new(err)))?;
    Ok(HistoryId::from_bytes(bytes))
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

    async fn migrate(&self) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let version: i64 =
            db::query_scalar("PRAGMA user_version").fetch_one(pool).await.map_err(store)?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }

        db::query("DROP TABLE IF EXISTS output_fts").execute(pool).await.map_err(store)?;
        db::query(
            "CREATE VIRTUAL TABLE output_fts USING fts5(history_id UNINDEXED, body, tokenize = \
             'unicode61')",
        )
        .execute(pool)
        .await
        .map_err(store)?;
        // `PRAGMA user_version` cannot be bound; the value is our own integer constant, so building
        // the statement text is injection-free (hence the `AssertSqlSafe`).
        db::query(sqlx::AssertSqlSafe(format!("PRAGMA user_version = {SCHEMA_VERSION}")))
            .execute(pool)
            .await
            .map_err(store)?;
        Ok(())
    }
}

impl Index for SqliteIndex {
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let key = id.into_bytes();
        // Capture is once-per-id, but reconcile/rebuild may re-run: replace any prior row so this
        // stays idempotent. FTS5 has no UNIQUE constraint to lean on, hence delete-then-insert.
        let mut tx = pool.begin().await.map_err(store)?;
        db::query("DELETE FROM output_fts WHERE history_id = ?")
            .bind(&key[..])
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        db::query("INSERT INTO output_fts(history_id, body) VALUES (?, ?)")
            .bind(&key[..])
            .bind_highlightable(self.highlighter, text)
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        tx.commit().await.map_err(store)?;
        Ok(())
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        let mut ids = ids.peekable();

        if ids.peek().is_none() {
            return Ok(());
        }

        let pool = self.db.pool();
        let mut tx = pool.begin().await.map_err(store)?;
        for id in ids {
            let key = id.into_bytes();
            db::query("DELETE FROM output_fts WHERE history_id = ?")
                .bind(&key[..])
                .execute(&mut *tx)
                .await
                .map_err(store)?;
        }
        tx.commit().await.map_err(store)?;

        Ok(())
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        let match_expr = sanitize_query(query);
        if match_expr.is_empty() {
            return ChunkedStream::empty();
        }

        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let highlighter = self.highlighter;
        // `highlight()` hands back the whole body with every match wrapped in the markers, which is
        // the one pass that yields both the full output and where the matches sit in it.
        let rows = db::query(
            "SELECT history_id, highlight(output_fts, 1, ?, ?) AS body, -bm25(output_fts) AS \
             score FROM output_fts WHERE output_fts MATCH ? ORDER BY score DESC LIMIT ?",
        )
        .bind_highlight(highlighter)
        .bind(&match_expr)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await;

        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => return ChunkedStream::from_chunks([vec![Err(store(err))]]),
        };

        let matches: Vec<Result<OutputMatch, IndexError>> = rows
            .into_iter()
            .map(move |row| {
                let raw: &[u8] = row.try_get("history_id").map_err(store)?;
                let body: &str = row.try_get("body").map_err(store)?;
                Ok(OutputMatch {
                    history_id: id_from_bytes(raw)?,
                    output: highlighter.as_highlighted(body.to_owned()),
                    score: row.try_get("score").map_err(store)?,
                })
            })
            .collect();

        ChunkedStream::from_items(matches, CHUNK)
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        let rows = db::query("SELECT history_id FROM output_fts").fetch_all(self.db.pool()).await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => return ChunkedStream::from_chunks([vec![Err(store(err))]]),
        };

        let ids: Vec<Result<HistoryId, IndexError>> = rows
            .into_iter()
            .map(|row| id_from_bytes(row.try_get::<&[u8], _>("history_id").map_err(store)?))
            .collect();

        ChunkedStream::from_items(ids, CHUNK)
    }
}

/// Turn free-form user input into a safe FTS5 `MATCH` expression: each whitespace-separated term
/// becomes a quoted string (doubling any embedded quote), ANDed together. Quoting sidesteps FTS5's
/// query syntax so punctuation in the query can never raise a syntax error.
fn sanitize_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 2);
    for term in query.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        for ch in term.chars() {
            if ch == '"' {
                out.push('"'); // FTS5 escapes an embedded double-quote by doubling it.
            }
            out.push(ch);
        }
        out.push('"');
    }
    out
}

#[cfg(test)]
mod tests {
    use futures::TryStreamExt;

    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*uuid::Uuid::from_u128(n).as_bytes())
    }

    async fn search_hits(index: &SqliteIndex, query: &str, limit: usize) -> Vec<OutputMatch> {
        index.search(query, limit).await.items().try_collect().await.expect("search")
    }

    async fn temp_index() -> (SqliteIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = SqliteIndex::open(&dir.path().join("index.sqlite")).await.expect("open");
        (index, dir)
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
        index.remove([hid(1)].into_iter()).await.expect("remove");
        assert!(search_hits(&index, "removable", 10).await.is_empty());
    }

    #[tokio::test]
    async fn indexed_ids_lists_every_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "one").await.expect("insert");
        index.insert(hid(2), "two").await.expect("insert");

        let mut ids: Vec<HistoryId> =
            index.indexed_ids().await.items().try_collect().await.expect("indexed_ids");
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, vec![hid(1), hid(2)]);
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
                .items()
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
}
