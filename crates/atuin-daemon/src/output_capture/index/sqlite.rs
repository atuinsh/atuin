use std::path::Path;

use atuin_client::history::HistoryId;
use atuin_common::db;
use atuin_common::db::sqlite::Sqlite;
use sqlx::Row;

use super::{Index, IndexError, OutputMatch};

/// Bump this whenever the on-disk index shape changes. On open a mismatch drops the table; the
/// fjall store is the source of truth, so the next reconcile repopulates it.
const SCHEMA_VERSION: i64 = 1;

/// How many tokens of context a result snippet carries around its match.
const SNIPPET_TOKENS: i64 = 32;

/// A full-text search index backed by a sidecar sqlite FTS5 table.
///
/// Cloning shares the underlying connection pool, so a read-only clone can be handed to the search
/// service while the facade keeps the writing handle.
#[derive(Debug, Clone)]
pub struct SqliteIndex {
    db: Sqlite,
}

fn store(err: sqlx::Error) -> IndexError {
    IndexError::Storage(Box::new(err))
}

fn parse_id(raw: &str) -> Result<HistoryId, IndexError> {
    uuid::Uuid::parse_str(raw)
        .map(|uuid| HistoryId::from_bytes(uuid.into_bytes()))
        .map_err(|err| IndexError::Storage(Box::new(err)))
}

impl SqliteIndex {
    /// Open (creating if missing) the index at `path`, running any pending migration.
    pub async fn open(path: &Path) -> Result<Self, IndexError> {
        let db = Sqlite::builder(path.as_os_str())
            .restrict_permissions()
            .open()
            .await
            .map_err(|err| IndexError::Storage(Box::new(err)))?;
        let index = Self { db };
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
            "CREATE VIRTUAL TABLE output_fts \
             USING fts5(history_id UNINDEXED, body, tokenize = 'unicode61')",
        )
        .execute(pool)
        .await
        .map_err(store)?;
        // `PRAGMA user_version` cannot be bound; the value is our own integer constant, so building
        // the statement text is injection-free (hence the `AssertSqlSafe`).
        db::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA user_version = {SCHEMA_VERSION}"
        )))
        .execute(pool)
        .await
        .map_err(store)?;
        Ok(())
    }
}

impl Index for SqliteIndex {
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let id = id.to_string();
        // Capture is once-per-id, but reconcile/rebuild may re-run: replace any prior row so this
        // stays idempotent. FTS5 has no UNIQUE constraint to lean on, hence delete-then-insert.
        let mut tx = pool.begin().await.map_err(store)?;
        db::query("DELETE FROM output_fts WHERE history_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        db::query("INSERT INTO output_fts(history_id, body) VALUES (?, ?)")
            .bind(&id)
            .bind(text)
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        tx.commit().await.map_err(store)?;
        Ok(())
    }

    async fn remove(&self, ids: &[HistoryId]) -> Result<(), IndexError> {
        if ids.is_empty() {
            return Ok(());
        }
        let pool = self.db.pool();
        let mut tx = pool.begin().await.map_err(store)?;
        for id in ids {
            db::query("DELETE FROM output_fts WHERE history_id = ?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(store)?;
        }
        tx.commit().await.map_err(store)?;
        Ok(())
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError> {
        let match_expr = sanitize_query(query);
        if match_expr.is_empty() {
            return Ok(Vec::new());
        }

        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = db::query(
            "SELECT history_id, \
                    snippet(output_fts, 1, '', '', '…', ?) AS snippet, \
                    -bm25(output_fts) AS score \
             FROM output_fts \
             WHERE output_fts MATCH ? \
             ORDER BY score DESC \
             LIMIT ?",
        )
        .bind(SNIPPET_TOKENS)
        .bind(&match_expr)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(store)?;

        rows.into_iter()
            .map(|row| {
                let raw: String = row.try_get("history_id").map_err(store)?;
                Ok(OutputMatch {
                    history_id: parse_id(&raw)?,
                    snippet: row.try_get("snippet").map_err(store)?,
                    score: row.try_get("score").map_err(store)?,
                })
            })
            .collect()
    }

    async fn indexed_ids(&self) -> Result<Vec<HistoryId>, IndexError> {
        let rows = db::query("SELECT history_id FROM output_fts")
            .fetch_all(self.db.pool())
            .await
            .map_err(store)?;
        rows.into_iter()
            .map(|row| parse_id(&row.try_get::<String, _>("history_id").map_err(store)?))
            .collect()
    }
}

/// Turn free-form user input into a safe FTS5 `MATCH` expression: each whitespace-separated term
/// becomes a quoted string (doubling any embedded quote), ANDed together. Quoting sidesteps FTS5's
/// query syntax so punctuation in the query can never raise a syntax error.
fn sanitize_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*uuid::Uuid::from_u128(n).as_bytes())
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

        let hits = index.search("error", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].history_id, hid(1));
        assert!(hits[0].snippet.contains("error"), "snippet shows the match: {:?}", hits[0]);
    }

    #[tokio::test]
    async fn search_returns_nothing_for_a_miss() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "hello world").await.expect("insert");
        assert!(index.search("absent", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn reinserting_the_same_id_does_not_duplicate() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "first text apple").await.expect("insert");
        index.insert(hid(1), "second text apple").await.expect("reinsert");

        let hits = index.search("apple", 10).await.expect("search");
        assert_eq!(hits.len(), 1, "the id appears once, not twice");
    }

    #[tokio::test]
    async fn remove_drops_the_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "removable content").await.expect("insert");
        index.remove(&[hid(1)]).await.expect("remove");
        assert!(index.search("removable", 10).await.expect("search").is_empty());
    }

    #[tokio::test]
    async fn indexed_ids_lists_every_entry() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "one").await.expect("insert");
        index.insert(hid(2), "two").await.expect("insert");

        let mut ids = index.indexed_ids().await.expect("indexed_ids");
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, vec![hid(1), hid(2)]);
    }

    #[tokio::test]
    async fn punctuation_in_the_query_does_not_error() {
        let (index, _dir) = temp_index().await;
        index.insert(hid(1), "a line with a path /usr/bin and a colon").await.expect("insert");
        // Bare FTS5 syntax chars would be a syntax error unquoted; sanitizing must swallow them.
        for q in ["\"unbalanced", "a OR", "path:", "(", "*", "-x"] {
            index.search(q, 10).await.unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
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
        let hits = index.search("persistent", 10).await.expect("search");
        assert_eq!(hits.len(), 1);
    }
}
