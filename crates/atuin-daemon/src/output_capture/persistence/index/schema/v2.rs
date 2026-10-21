use std::sync::Arc;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::{FtsQueryExt, TextHighlighter, match_expression};
use atuin_common::db::{self};
use atuin_common::futures::stream::ChunkedStream;
use futures::stream;
use sqlx::Row;

use super::super::{HighlightedCapture, IndexError, RankedMatch};
use super::{CHUNK, id_from_bytes, store};

pub struct Schema;

impl super::Schema for Schema {
    const VERSION: i64 = 2;

    async fn setup(db: &Sqlite) -> Result<(), IndexError> {
        let pool = db.pool();
        for statement in [
            "DROP TABLE IF EXISTS output_fts",
            "DROP TABLE IF EXISTS indexed",
            "CREATE TABLE indexed(id INTEGER PRIMARY KEY, history_id BLOB NOT NULL UNIQUE)",
            // A capture's kept head and tail go in separate columns because FTS5 matches a phrase
            // within one column only, and the words either side of a discarded middle were never
            // next to each other.
            "CREATE VIRTUAL TABLE output_fts USING fts5(head, tail, content = '', \
             contentless_delete = 1, tokenize = 'unicode61')",
        ] {
            db::query(statement).execute(pool).await.map_err(store)?;
        }
        Ok(())
    }

    async fn insert(
        db: &Sqlite,
        highlighter: TextHighlighter,
        id: HistoryId,
        head: &str,
        tail: Option<&str>,
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
        db::query("INSERT OR REPLACE INTO output_fts(rowid, head, tail) VALUES (?, ?, ?)")
            .bind(rowid)
            .bind_highlightable(highlighter, head)
            .bind_highlightable(highlighter, tail.unwrap_or_default())
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        tx.commit().await.map_err(store)?;
        Ok(())
    }

    async fn remove(db: &Sqlite, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        let keys: Vec<[u8; 16]> = ids.map(HistoryId::into_bytes).collect();
        if keys.is_empty() {
            return Ok(());
        }

        // A chunk of ids per statement rather than two statements per id: a bulk history delete
        // passes every id, most of which never had output indexed, and 400k single-row
        // statements made removing 200k ids take ~20s.
        let keys_per_delete = db.info().await.variable_number_limit().max(1);
        Self::remove_chunked(db, &keys, keys_per_delete).await
    }

    async fn search(
        db: &Sqlite,
        terms: impl IntoIterator<Item = &str>,
        limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        let pool = db.pool().clone();
        let terms: Arc<[String]> = terms.into_iter().map(str::to_owned).collect();

        ChunkedStream::new(stream::unfold(Some(0usize), move |offset| {
            let pool = pool.clone();
            let terms = terms.clone();
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
                let stmt = stmt.bind_match_query(terms.iter())?;
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
        terms: impl IntoIterator<Item = &str>,
        head: &str,
        tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError> {
        let Some(expr) = match_expression(terms) else {
            return Ok(HighlightedCapture::unmarked(highlighter, head, tail));
        };

        // The index is contentless, so sqlite's `highlight()` cannot run against it; and
        // highlighting by hand would not match how sqlite tokenizes (unicode61 folds case and
        // diacritics: 'cafe' matches café). So the body goes through a scratch FTS5 table, where
        // the same MATCH highlights it exactly as it was matched.
        let mut conn = db.pool().acquire().await.map_err(store)?;
        db::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS temp.highlights USING fts5(head, tail, tokenize = \
             'unicode61')",
        )
        .execute(&mut *conn)
        .await
        .map_err(store)?;
        db::query("DELETE FROM temp.highlights").execute(&mut *conn).await.map_err(store)?;
        db::query("INSERT INTO temp.highlights(rowid, head, tail) VALUES (1, ?, ?)")
            .bind_highlightable(highlighter, head)
            .bind_highlightable(highlighter, tail.unwrap_or_default())
            .execute(&mut *conn)
            .await
            .map_err(store)?;
        let highlighted = db::query(
            "SELECT highlight(highlights, 0, ?, ?), highlight(highlights, 1, ?, ?) FROM \
             temp.highlights WHERE highlights MATCH ?",
        )
        .bind_highlight(highlighter)
        .bind_highlight(highlighter)
        .bind(expr)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store)?;

        // A body that no longer matches (its plaintext changed since it was indexed) is still a
        // hit, just an unhighlighted one.
        let Some(row) = highlighted else {
            return Ok(HighlightedCapture::unmarked(highlighter, head, tail));
        };
        let marked_head: String = row.try_get(0).map_err(store)?;
        let marked_tail: String = row.try_get(1).map_err(store)?;
        Ok(HighlightedCapture {
            head: highlighter.as_highlighted(marked_head),
            tail: tail.map(|_| highlighter.as_highlighted(marked_tail)),
        })
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

impl Schema {
    /// [`super::Schema::remove`]'s work, `keys_per_delete` ids per statement, in one transaction.
    /// Separate so tests can cross chunk boundaries without tens of thousands of ids.
    pub(crate) async fn remove_chunked(
        db: &Sqlite,
        keys: &[[u8; 16]],
        keys_per_delete: usize,
    ) -> Result<(), IndexError> {
        let pool = db.pool();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(store)?;
        for chunk in keys.chunks(keys_per_delete.max(1)) {
            let mut fts = sqlx::QueryBuilder::new(
                "DELETE FROM output_fts WHERE rowid IN (SELECT id FROM indexed WHERE history_id \
                 IN (",
            );
            let mut list = fts.separated(", ");
            for key in chunk {
                list.push_bind(&key[..]);
            }
            fts.push("))");
            fts.build().execute(&mut *tx).await.map_err(store)?;

            let mut indexed = sqlx::QueryBuilder::new("DELETE FROM indexed WHERE history_id IN (");
            let mut list = indexed.separated(", ");
            for key in chunk {
                list.push_bind(&key[..]);
            }
            indexed.push(")");
            indexed.build().execute(&mut *tx).await.map_err(store)?;
        }
        tx.commit().await.map_err(store)?;

        Ok(())
    }
}
