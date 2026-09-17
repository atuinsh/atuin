use std::collections::HashMap;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::{FtsQueryExt, TextHighlighter, match_expression};
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;
use atuin_common::string::highlighted::HighlightedString;
use easy_cast::Conv;
use futures::stream;
use sqlx::Row;

use super::super::{IndexError, RankedMatch};
use super::{CHUNK, id_from_bytes, store};

pub struct Schema;

/// Bodies highlighted per scratch-table round. Each can be up to `max_output_size`, so this is
/// deliberately far below the other chunk sizes.
impl Schema {
    async fn highlight_all(
        db: &Sqlite,
        highlighter: TextHighlighter,
        expr: &str,
        bodies: impl IntoIterator<Item = impl AsRef<str>> + Send,
    ) -> Result<Vec<HighlightedString>, IndexError> {
        // Okay this looks so confusing if you're reading this for the first-time, so let me guide
        // you through the reasoning here.
        //
        // The Sqlite index doesn't contain the full text output. It merely contains an index of the
        // text that was indexed, but not the text itself.
        //
        // This means that we can't use the Sqlite `highlight(...)` function naively, which is what
        // we'd really like to use at the end of the day.
        //
        // Now, you'd think that we can use the sqlite index to find the history ID, get the output
        // capture and then _manually_ highlight the text the user searched for.
        //
        // However, this unfortunately doesn't highlight as Sqlite would highlight -- they do
        // something called 'unicode61', which matches stuff like café for a search string 'cafe',
        // so the only way to *exactly* present the Sqlite highlighting is to, well, go through
        // Sqlite.
        //
        // So here's what we do -- we create a new `temp.highlight` table, we stick entries in it,
        // we then immediately invoke MATCH with a `highlight` on the same call the user made.
        //
        // Hacky and dirty, but works!
        let bodies: Vec<_> = bodies.into_iter().collect();
        let mut conn = db.pool().acquire().await.map_err(store)?;
        db::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS temp.highlights USING fts5(body, tokenize = \
             'unicode61')",
        )
        .execute(&mut *conn)
        .await
        .map_err(store)?;
        db::query("DELETE FROM temp.highlights").execute(&mut *conn).await.map_err(store)?;
        for (n, body) in bodies.iter().enumerate() {
            db::query("INSERT INTO temp.highlights(rowid, body) VALUES (?, ?)")
                .bind(i64::conv(n))
                .bind_highlightable(highlighter, body.as_ref())
                .execute(&mut *conn)
                .await
                .map_err(store)?;
        }
        let mut highlighted: HashMap<i64, String> = db::query(
            "SELECT rowid, highlight(highlights, 0, ?, ?) AS body FROM temp.highlights WHERE \
             highlights MATCH ?",
        )
        .bind_highlight(highlighter)
        .bind(expr)
        .fetch_all(&mut *conn)
        .await
        .map_err(store)?
        .into_iter()
        .map(|row| Ok((row.try_get("rowid").map_err(store)?, row.try_get("body").map_err(store)?)))
        .collect::<Result<_, IndexError>>()?;

        Ok(bodies
            .into_iter()
            .enumerate()
            .map(|(n, body)| {
                // A body that no longer matches (its plaintext changed since it was indexed) is
                // still a hit, just an unhighlighted one.
                highlighter.as_highlighted(
                    highlighted
                        .remove(&i64::conv(n))
                        .unwrap_or_else(|| highlighter.sanitize(body.as_ref()).into_owned()),
                )
            })
            .collect())
    }
}

impl super::Schema for Schema {
    const VERSION: i64 = 1;

    async fn setup(db: &Sqlite) -> Result<(), IndexError> {
        let pool = db.pool();
        for statement in [
            "DROP TABLE IF EXISTS output_fts",
            "DROP TABLE IF EXISTS indexed",
            "CREATE TABLE indexed(id INTEGER PRIMARY KEY, history_id BLOB NOT NULL UNIQUE)",
            "CREATE VIRTUAL TABLE output_fts USING fts5(body, content = '', contentless_delete = \
             1, tokenize = 'unicode61')",
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
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(store)?;
        let rowid: i64 = db::query_scalar(
            "INSERT INTO indexed(history_id) VALUES (?) ON CONFLICT(history_id) DO UPDATE SET \
             history_id = excluded.history_id RETURNING id",
        )
        .bind(&key[..])
        .fetch_one(&mut *tx)
        .await
        .map_err(store)?;
        db::query("INSERT OR REPLACE INTO output_fts(rowid, body) VALUES (?, ?)")
            .bind(rowid)
            .bind_highlightable(highlighter, text)
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
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(store)?;
        for id in ids {
            let key = id.into_bytes();
            db::query(
                "DELETE FROM output_fts WHERE rowid = (SELECT id FROM indexed WHERE history_id = \
                 ?)",
            )
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
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        let pool = db.pool().clone();
        let query = query.to_owned();

        ChunkedStream::new(stream::unfold(Some(0usize), move |offset| {
            let pool = pool.clone();
            let query = query.clone();
            async move {
                let offset = offset?;
                let want = if limit == 0 {
                    CHUNK.get()
                } else {
                    limit.saturating_sub(offset).min(CHUNK.get())
                };
                if want == 0 {
                    return None;
                }

                let stmt = db::query(
                    "SELECT indexed.history_id, -bm25(output_fts) AS score FROM output_fts JOIN \
                     indexed ON indexed.id = output_fts.rowid WHERE output_fts MATCH ? ORDER BY \
                     score DESC, indexed.history_id LIMIT ? OFFSET ?",
                );
                let stmt = stmt.bind_match_query(&query)?;
                let page = i64::try_from(want).unwrap_or(i64::MAX);
                let skip = i64::try_from(offset).unwrap_or(i64::MAX);
                let rows = match stmt.bind(page).bind(skip).fetch_all(&pool).await {
                    Ok(rows) => rows,
                    Err(err) => return Some((vec![Err(store(err))], None)),
                };
                if rows.is_empty() {
                    return None;
                }

                let got = rows.len();
                let chunk: Vec<Result<RankedMatch, IndexError>> = rows
                    .into_iter()
                    .map(|row| {
                        Ok(RankedMatch {
                            history_id: id_from_bytes(row.try_get("history_id").map_err(store)?)?,
                            score: row.try_get("score").map_err(store)?,
                        })
                    })
                    .collect();

                // A short page means the index is exhausted; yield it, then stop.
                let next = (got == want).then_some(offset + got);
                Some((chunk, next))
            }
        }))
    }

    async fn highlight(
        db: &Sqlite,
        highlighter: TextHighlighter,
        query: &str,
        bodies: impl IntoIterator<Item = impl AsRef<str>> + Send,
    ) -> Result<Vec<HighlightedString>, IndexError> {
        match match_expression(query) {
            Some(expr) => Self::highlight_all(db, highlighter, &expr, bodies).await,
            None => Ok(bodies
                .into_iter()
                .map(|body| {
                    highlighter.as_highlighted(highlighter.sanitize(body.as_ref()).into_owned())
                })
                .collect()),
        }
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
