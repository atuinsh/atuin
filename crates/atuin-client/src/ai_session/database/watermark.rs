//! How far the sidecar has been reprojected from the record store, per record series.
//!
//! Reprojection replays a series in idx order and only moves its watermark once the appends
//! below it have committed. Those appends and the watermark live in the same WAL database, so a
//! watermark that survives a crash implies every append it covers does too; a crash in between
//! merely replays some records, which `append`'s `ON CONFLICT` keying makes harmless.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::{self};
use atuin_domain::record::{HostId, RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use tracing::warn;
use uuid::Uuid;

use super::{AiSessionDatabase, DbError};

/// The last record of a series the sidecar has projected, and every one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watermark {
    pub idx: RecordIdx,
    pub record_id: RecordId,
}

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

    /// Move `series`' watermark from `from` to `to`, only if it still is `from`. Returns false
    /// when it was changed meanwhile (typically cleared by an invalidation), leaving it alone so
    /// the next reprojection starts over rather than skipping what the invalidation asked for.
    ///
    /// Call only once the appends of every record up to `to` have committed.
    pub async fn advance_reproject_watermark(
        &self,
        series: &RecordSeriesKey,
        from: Option<Watermark>,
        to: Watermark,
    ) -> Result<bool, DbError> {
        let host = series.host_id.as_hyphenated().to_string();
        let idx = i64::try_from(to.idx).unwrap_or(i64::MAX);
        let record_id = to.record_id.as_hyphenated().to_string();

        let done = match from {
            Some(from) => {
                db::query(
                    "UPDATE reproject_watermark SET idx = ?, record_id = ? WHERE host = ? AND tag \
                     = ? AND idx = ? AND record_id = ?",
                )
                .bind(idx)
                .bind(record_id)
                .bind(host)
                .bind(series.tag.as_str())
                .bind(i64::try_from(from.idx).unwrap_or(i64::MAX))
                .bind(from.record_id.as_hyphenated().to_string())
                .execute(self.db.pool())
                .await?
            }
            None => {
                db::query(
                    "INSERT INTO reproject_watermark (host, tag, idx, record_id) VALUES (?, ?, ?, \
                     ?) ON CONFLICT(host, tag) DO NOTHING",
                )
                .bind(host)
                .bind(series.tag.as_str())
                .bind(idx)
                .bind(record_id)
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
        db::query("DELETE FROM reproject_watermark WHERE host = ? AND tag = ?")
            .bind(series.host_id.as_hyphenated().to_string())
            .bind(series.tag.as_str())
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// Forget every watermark: the next reprojection replays the whole record store.
    pub async fn clear_reproject_watermarks(&self) -> Result<(), DbError> {
        db::query("DELETE FROM reproject_watermark").execute(self.db.pool()).await?;
        Ok(())
    }

    /// Make the next reprojection into the sidecar at `path` a full one, without migrating or
    /// otherwise opening it for use. For the maintenance commands that rewrite the record store
    /// while the daemon (which owns the sidecar) may be running. A missing sidecar, or one from
    /// before watermarks, is replayed in full anyway.
    pub async fn invalidate_projection(path: impl AsRef<Path>) -> Result<(), DbError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(());
        }

        let sqlite = Sqlite::builder(path.as_os_str()).restrict_permissions().open().await?;
        let exists: bool = db::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = \
             'reproject_watermark')",
        )
        .fetch_one(sqlite.pool())
        .await?;
        if exists {
            db::query("DELETE FROM reproject_watermark").execute(sqlite.pool()).await?;
        }
        sqlite.pool().close().await;

        Ok(())
    }
}
