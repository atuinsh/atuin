use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::{FtsQueryExt, TextHighlighter};
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;
use futures::stream;
use sqlx::Row;

use super::super::{IndexError, OutputMatch};
use super::{CHUNK, id_from_bytes, store};

pub struct Schema;

impl super::Schema for Schema {
    const VERSION: i64 = 1;

    async fn setup(db: &Sqlite) -> Result<(), IndexError> {
        let pool = db.pool();
        for statement in [
            "DROP TABLE IF EXISTS output_fts",
            "DROP TABLE IF EXISTS indexed",
            "CREATE VIRTUAL TABLE output_fts USING fts5(history_id UNINDEXED, body, tokenize = \
             'unicode61')",
            "CREATE TABLE indexed(history_id BLOB PRIMARY KEY) WITHOUT ROWID",
        ] {
            db::query(statement).execute(pool).await.map_err(store)?;
        }
        Ok(())
    }

    async fn insert(
        db: &Sqlite,
        highlighter: TextHighlighter,
        id: HistoryId,
        text: &str,
    ) -> Result<(), IndexError> {
        let pool = db.pool();
        let key = id.into_bytes();
        let mut tx = pool.begin().await.map_err(store)?;
        db::query("DELETE FROM output_fts WHERE history_id = ?")
            .bind(&key[..])
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        db::query("INSERT INTO output_fts(history_id, body) VALUES (?, ?)")
            .bind(&key[..])
            .bind_highlightable(highlighter, text)
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        db::query("INSERT OR IGNORE INTO indexed(history_id) VALUES (?)")
            .bind(&key[..])
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        tx.commit().await.map_err(store)?;
        Ok(())
    }

    async fn remove(db: &Sqlite, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        let mut ids = ids.peekable();

        if ids.peek().is_none() {
            return Ok(());
        }

        let pool = db.pool();
        let mut tx = pool.begin().await.map_err(store)?;
        for id in ids {
            let key = id.into_bytes();
            db::query("DELETE FROM output_fts WHERE history_id = ?")
                .bind(&key[..])
                .execute(&mut *tx)
                .await
                .map_err(store)?;
            db::query("DELETE FROM indexed WHERE history_id = ?")
                .bind(&key[..])
                .execute(&mut *tx)
                .await
                .map_err(store)?;
        }
        tx.commit().await.map_err(store)?;

        Ok(())
    }

    async fn search(
        db: &Sqlite,
        highlighter: TextHighlighter,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let stmt = db::query(
            "SELECT history_id, highlight(output_fts, 1, ?, ?) AS body, -bm25(output_fts) AS \
             score FROM output_fts WHERE output_fts MATCH ? ORDER BY score DESC LIMIT ?",
        )
        .bind_highlight(highlighter);
        let Some(stmt) = stmt.bind_match_query(query) else {
            return ChunkedStream::empty();
        };
        let rows = stmt.bind(limit).fetch_all(db.pool()).await;

        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => return ChunkedStream::from_error(store(err)),
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

    async fn indexed_ids(db: &Sqlite) -> ChunkedStream<Result<HistoryId, IndexError>> {
        let pool = db.pool().clone();
        let page = i64::try_from(CHUNK.get()).unwrap_or(i64::MAX);

        ChunkedStream::new(stream::unfold(Some(Vec::new()), move |after| {
            let pool = pool.clone();
            async move {
                let after: Vec<u8> = after?;

                let rows = match db::query(
                    "SELECT history_id FROM indexed WHERE history_id > ? ORDER BY history_id \
                     LIMIT ?",
                )
                .bind(after)
                .bind(page)
                .fetch_all(&pool)
                .await
                {
                    Ok(rows) => rows,
                    Err(err) => return Some((vec![Err(store(err))], None)),
                };

                let next = match rows.last().map(|row| row.try_get::<Vec<u8>, _>("history_id")) {
                    None => return None,
                    Some(Ok(bytes)) => Some(bytes),
                    Some(Err(err)) => return Some((vec![Err(store(err))], None)),
                };

                let chunk: Vec<Result<HistoryId, IndexError>> = rows
                    .into_iter()
                    .map(|row| id_from_bytes(row.try_get::<&[u8], _>("history_id").map_err(store)?))
                    .collect();

                Some((chunk, next))
            }
        }))
    }
}
