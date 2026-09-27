//! How far the sidecar has been reprojected from the record store, per record series.
//!
//! Reprojection replays a series in idx order and only moves its watermark once the appends
//! below it have committed. Those appends and the watermark live in the same WAL database, so a
//! watermark that survives a crash implies every append it covers does too; a crash in between
//! merely replays some records, which `append`'s `ON CONFLICT` keying makes harmless.
//!
//! Anything that clears watermarks or deletes projected rows is an invalidation, and bumps the
//! [`Generation`] in the same transaction. A reprojection moves a watermark only while the
//! generation is the one it read before replaying, so it never records as projected what an
//! invalidation removed or asked to be replayed meanwhile (see migration 0004).

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::{self};
use atuin_domain::record::{HostId, RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use sqlx::SqliteConnection;
use tracing::warn;
use uuid::Uuid;

use super::{AiSessionDatabase, DbError};

/// The last record of a series the sidecar has projected, and every one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watermark {
    pub idx: RecordIdx,
    pub record_id: RecordId,
}

/// How many invalidations the sidecar has seen. Read one before replaying records, and pass it
/// to [`AiSessionDatabase::advance_reproject_watermark`]: the watermark moves only if no
/// invalidation happened in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(i64);

impl AiSessionDatabase {
    /// Every series' watermark. A series without one must be replayed from its first record.
    pub async fn reproject_watermarks(
        &self,
    ) -> Result<HashMap<RecordSeriesKey, Watermark>, DbError> {
        let rows: Vec<(String, String, i64, String)> =
            db::query_as("SELECT host, tag, idx, record_id FROM reproject_watermark")
                .fetch_all(self.db.pool())
                .await?;

        let mut marks = HashMap::with_capacity(rows.len());
        for (host, tag, idx, record_id) in rows {
            // An unreadable row is as good as none: the series is replayed and the row rewritten.
            let (Ok(host), Ok(record_id), Ok(idx)) =
                (Uuid::from_str(&host), Uuid::from_str(&record_id), u64::try_from(idx))
            else {
                warn!(host, tag, idx, record_id, "ignoring malformed reprojection watermark");
                continue;
            };
            marks.insert(RecordSeriesKey::new(HostId(host), RecordTag::from(tag)), Watermark {
                idx,
                record_id: RecordId(record_id),
            });
        }

        Ok(marks)
    }

    /// The current invalidation [`Generation`].
    pub async fn projection_generation(&self) -> Result<Generation, DbError> {
        let generation: i64 =
            db::query_scalar("SELECT generation FROM projection_state WHERE id = 0")
                .fetch_one(self.db.pool())
                .await?;
        Ok(Generation(generation))
    }

    /// Move `series`' watermark from `from` to `to`, only if it still is `from` and no
    /// invalidation happened since `generation` was read. Returns false otherwise, leaving it
    /// alone: the caller must start that series over rather than skip what the invalidation
    /// asked for.
    ///
    /// Call only once the appends of every record up to `to` have committed.
    pub async fn advance_reproject_watermark(
        &self,
        series: &RecordSeriesKey,
        generation: Generation,
        from: Option<Watermark>,
        to: Watermark,
    ) -> Result<bool, DbError> {
        let host = series.host_id.as_hyphenated().to_string();
        let idx = i64::try_from(to.idx).unwrap_or(i64::MAX);
        let record_id = to.record_id.as_hyphenated().to_string();

        // Each statement tests the generation as it writes, so an invalidation lands either
        // before it (and the write does nothing) or after it (and clears what it wrote).
        let done = match from {
            Some(from) => {
                db::query(
                    "UPDATE reproject_watermark SET idx = ?, record_id = ? WHERE host = ? AND tag \
                     = ? AND idx = ? AND record_id = ? AND (SELECT generation FROM \
                     projection_state WHERE id = 0) = ?",
                )
                .bind(idx)
                .bind(record_id)
                .bind(host)
                .bind(series.tag.as_str())
                .bind(i64::try_from(from.idx).unwrap_or(i64::MAX))
                .bind(from.record_id.as_hyphenated().to_string())
                .bind(generation.0)
                .execute(self.db.pool())
                .await?
            }
            None => {
                // The `WHERE` also settles the upsert's parsing ambiguity after a `SELECT`.
                db::query(
                    "INSERT INTO reproject_watermark (host, tag, idx, record_id) SELECT ?, ?, ?, \
                     ? WHERE (SELECT generation FROM projection_state WHERE id = 0) = ? ON \
                     CONFLICT(host, tag) DO NOTHING",
                )
                .bind(host)
                .bind(series.tag.as_str())
                .bind(idx)
                .bind(record_id)
                .bind(generation.0)
                .execute(self.db.pool())
                .await?
            }
        };

        Ok(done.rows_affected() == 1)
    }

    /// Forget `series`' watermark, so it is replayed from its first record.
    pub async fn forget_reproject_watermark(
        &self,
        series: &RecordSeriesKey,
    ) -> Result<(), DbError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        db::query("DELETE FROM reproject_watermark WHERE host = ? AND tag = ?")
            .bind(series.host_id.as_hyphenated().to_string())
            .bind(series.tag.as_str())
            .execute(&mut *tx)
            .await?;
        Self::bump_generation(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Forget every watermark: the next reprojection replays the whole record store.
    pub async fn clear_reproject_watermarks(&self) -> Result<(), DbError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        db::query("DELETE FROM reproject_watermark").execute(&mut *tx).await?;
        Self::bump_generation(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Make sure the watermarks were made with the key whose PASERK id is `key_id`, clearing
    /// them all when they were not (or when that is unknown, as right after migration 0004).
    /// Returns whether it cleared them.
    ///
    /// A record the key cannot decrypt holds its series' watermark below it, so a stale key
    /// never moves a watermark past what the right one could project. This covers the rest: once
    /// the key changes, everything is replayed with the new one.
    pub async fn check_projection_key(&self, key_id: &str) -> Result<bool, DbError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let stored: Option<String> =
            db::query_scalar("SELECT key_id FROM projection_state WHERE id = 0")
                .fetch_one(&mut *tx)
                .await?;
        if stored.as_deref() == Some(key_id) {
            return Ok(false);
        }

        db::query("DELETE FROM reproject_watermark").execute(&mut *tx).await?;
        db::query("UPDATE projection_state SET key_id = ? WHERE id = 0")
            .bind(key_id)
            .execute(&mut *tx)
            .await?;
        Self::bump_generation(&mut tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Record an invalidation, inside the transaction making it.
    pub(super) async fn bump_generation(conn: &mut SqliteConnection) -> Result<(), DbError> {
        db::query("UPDATE projection_state SET generation = generation + 1 WHERE id = 0")
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Make the next reprojection into the sidecar at `path` a full one, without migrating or
    /// otherwise opening it for use. For the maintenance commands that rewrite the record store
    /// while the daemon (which owns the sidecar) may be running: a reprojection the daemon has in
    /// flight notices, and starts over. A missing sidecar, or one from before watermarks, is
    /// replayed in full anyway.
    pub async fn invalidate_projection(path: impl AsRef<Path>) -> Result<(), DbError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(());
        }

        let sqlite = Sqlite::builder(path.as_os_str()).restrict_permissions().open().await?;
        let result = Self::invalidate_tables(&sqlite).await;
        sqlite.pool().close().await;
        result
    }

    /// [`Self::invalidate_projection`] over an open sidecar, at whatever schema version it is.
    async fn invalidate_tables(sqlite: &Sqlite) -> Result<(), DbError> {
        let mut tx = sqlite.pool().begin_with("BEGIN IMMEDIATE").await?;
        let tables: Vec<String> = db::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN \
             ('reproject_watermark', 'projection_state')",
        )
        .fetch_all(&mut *tx)
        .await?;
        if tables.iter().any(|t| t == "reproject_watermark") {
            db::query("DELETE FROM reproject_watermark").execute(&mut *tx).await?;
        }
        if tables.iter().any(|t| t == "projection_state") {
            Self::bump_generation(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
