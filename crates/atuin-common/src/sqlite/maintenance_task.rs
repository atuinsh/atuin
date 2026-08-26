//! Utility which compacts a SQLite database.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use thiserror::Error;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::sqlite::Info;
use crate::sync::EagerFutureCell;

#[derive(Debug, Error)]
enum MaintenanceError {
    #[error("failed to compact the WAL due to a sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("failed to compact the WAL due to an IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct ActiveMaintenanceTask {
    _task: Arc<AbortOnDropHandle<()>>,
}

impl ActiveMaintenanceTask {
    // How many bytes does the WAL need to reach before it is force-compacted.
    // Normally, SQLite will try to keep its WAL around 4MB.
    //
    // The guard we have here is for when something **really awry** is going on, so the limit is
    // relatively high.
    const THRESHOLD_BYTES: u64 = 32 * 1024 * 1024;

    // How often to we check to force compact the WAL?
    const PERIOD: Duration = Duration::from_mins(1);

    // What is the maximum acceptable period to compact the WAL?
    const MAX_TIMEOUT: Duration = Duration::from_millis(500);

    // How often the vacuum check runs. Deliberately far slower than the WAL check.
    const VACUUM_PERIOD: Duration = Duration::from_hours(6);

    // Delay before the first vacuum check, so we never vacuum during startup.
    const VACUUM_INITIAL_DELAY: Duration = Duration::from_mins(5);

    // Reclaim only when at least this fraction of the file is free pages.
    const VACUUM_FREE_RATIO_THRESHOLD: f64 = 0.20;

    // ...and only when at least this many bytes are actually reclaimable.
    const VACUUM_MIN_RECLAIMABLE_BYTES: i64 = 8 * 1024 * 1024;

    // How many freelist pages to reclaim per incremental_vacuum call, keeping each write lock short.
    const INCREMENTAL_VACUUM_CHUNK_PAGES: i64 = 256;

    fn should_reclaim(page_count: i64, freelist: i64, page_size: i64) -> bool {
        if page_count <= 0 {
            return false;
        }

        let free_ratio = freelist as f64 / page_count as f64;
        let reclaimable = freelist.saturating_mul(page_size);

        free_ratio >= Self::VACUUM_FREE_RATIO_THRESHOLD
            && reclaimable >= Self::VACUUM_MIN_RECLAIMABLE_BYTES
    }

    fn spawn(conn: SqliteConnection, info: EagerFutureCell<Info>) -> Self {
        let task = tokio::spawn(Self::run(conn, info));

        Self {
            _task: Arc::new(AbortOnDropHandle::new(task)),
        }
    }

    async fn run(mut conn: SqliteConnection, info: EagerFutureCell<Info>) {
        let wal_path = match info.get().await.wal_path().map(Path::to_path_buf) {
            Ok(wal_path) => wal_path,
            Err(error) => {
                warn!(%error, "could not resolve the WAL path; WAL compactor disabled");
                return;
            }
        };

        let wal = match tokio::fs::File::open(&wal_path).await {
            Ok(wal) => wal,
            Err(error) => {
                warn!(%error, "could not open the WAL file; WAL compactor disabled");
                return;
            }
        };

        let mut compaction_ticker = tokio::time::interval(Self::PERIOD);
        // Consume the immediate first tick so the first WAL check happens after one PERIOD,
        // preserving the previous behavior.
        compaction_ticker.tick().await;

        // Fire the first vacuum check after an initial delay (never during startup), then
        // every VACUUM_PERIOD. interval_at's first tick lands at the deadline, not immediately.
        let mut vacuum_ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + Self::VACUUM_INITIAL_DELAY,
            Self::VACUUM_PERIOD,
        );

        loop {
            tokio::select! {
                _ = compaction_ticker.tick() => {
                    if let Err(error) = Self::compact_wal(&mut conn, &wal).await {
                        warn!(%error, "failed to compact the WAL");
                    }
                }
                _ = vacuum_ticker.tick() => {
                    if let Err(error) = Self::maintain_vacuum(&mut conn).await {
                        warn!(%error, "failed to vacuum the database");
                    }
                }
            }
        }
    }

    /// Check if WAL compaction is necessary, and, if so, perform WAL compaction.
    ///
    /// Under normal operation, SQLite automatically comapcts the WAL. Under very high reader
    /// contention, SQLite will skip compacting the WAL.
    ///
    /// SQLite allows for an explicit request to compact the WAL which will acquire a writer lock,
    /// pause all readers, and proceed with WAL compaction.
    async fn compact_wal(
        conn: &mut SqliteConnection,
        wal: &tokio::fs::File,
    ) -> Result<(), MaintenanceError> {
        let meta = wal.metadata().await?;
        if meta.len() < Self::THRESHOLD_BYTES {
            return Ok(());
        }

        let (busy, log, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(PASSIVE)").fetch_one(&mut *conn).await?;

        if busy != 0 || checkpointed < log {
            // This query risks causing reader starvation, but this is the intent.
            sqlx::query("PRAGMA wal_checkpoint(RESTART)").execute(conn).await?;
        }

        Ok(())
    }

    /// Ensure the database is in `auto_vacuum=INCREMENTAL`.
    ///
    /// Existing (field) databases default to `NONE`; switching modes on a populated database
    /// requires a full `VACUUM`. Returns `true` iff a conversion VACUUM was performed.
    async fn ensure_incremental_mode(
        conn: &mut SqliteConnection,
    ) -> Result<bool, MaintenanceError> {
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum").fetch_one(&mut *conn).await?;

        // 2 == INCREMENTAL; already done.
        if mode == 2 {
            return Ok(false);
        }

        sqlx::query("PRAGMA auto_vacuum = INCREMENTAL").execute(&mut *conn).await?;
        sqlx::query("VACUUM").execute(&mut *conn).await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&mut *conn).await?;

        Ok(true)
    }

    /// Returns `(page_count, freelist_count, page_size)`.
    async fn free_space_stats(
        conn: &mut SqliteConnection,
    ) -> Result<(i64, i64, i64), MaintenanceError> {
        let page_count: i64 =
            sqlx::query_scalar("PRAGMA page_count").fetch_one(&mut *conn).await?;
        let freelist: i64 =
            sqlx::query_scalar("PRAGMA freelist_count").fetch_one(&mut *conn).await?;
        let page_size: i64 =
            sqlx::query_scalar("PRAGMA page_size").fetch_one(&mut *conn).await?;

        Ok((page_count, freelist, page_size))
    }

    /// Reclaim the entire freelist in bounded chunks, then push freed space to disk.
    async fn drain_freelist(conn: &mut SqliteConnection) -> Result<(), MaintenanceError> {
        let chunk = Self::INCREMENTAL_VACUUM_CHUNK_PAGES;
        let sql = format!("PRAGMA incremental_vacuum({chunk})");

        loop {
            let before: i64 =
                sqlx::query_scalar("PRAGMA freelist_count").fetch_one(&mut *conn).await?;
            if before == 0 {
                break;
            }

            sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).execute(&mut *conn).await?;

            let after: i64 =
                sqlx::query_scalar("PRAGMA freelist_count").fetch_one(&mut *conn).await?;
            if after >= before {
                // No progress (e.g. not actually in incremental mode). Stop rather than spin.
                warn!("incremental_vacuum made no progress; stopping freelist drain");
                break;
            }
        }

        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&mut *conn).await?;

        Ok(())
    }

    /// Reclaim free space if fragmentation has crossed the thresholds.
    async fn reclaim_if_needed(conn: &mut SqliteConnection) -> Result<(), MaintenanceError> {
        let (page_count, freelist, page_size) = Self::free_space_stats(conn).await?;
        if !Self::should_reclaim(page_count, freelist, page_size) {
            return Ok(());
        }

        Self::drain_freelist(conn).await
    }

    /// One vacuum-maintenance cycle: convert legacy databases once, otherwise reclaim on threshold.
    async fn maintain_vacuum(conn: &mut SqliteConnection) -> Result<(), MaintenanceError> {
        // A conversion runs a full VACUUM which already reclaims everything, so skip the
        // incremental pass on the cycle that converts.
        if Self::ensure_incremental_mode(conn).await? {
            return Ok(());
        }

        Self::reclaim_if_needed(conn).await
    }

    async fn connect(opts: SqliteConnectOptions) -> Result<SqliteConnection, MaintenanceError> {
        let conn = SqliteConnection::connect_with(&opts.busy_timeout(Self::MAX_TIMEOUT)).await?;

        Ok(conn)
    }
}

#[derive(Debug, Clone)]
enum MaintenanceTaskInner {
    Active {
        _compactor: ActiveMaintenanceTask,
    },
    Inactive,
}

/// Compactor manages a background task which compacts the WAL as it grows out of hand.
///
/// Normally, Sqlite can compact itself without any issue, but certain extremely adversarial
/// workloads can cause compaction to fail. Readers are prioritized in sqlite over the compaction
/// process (called "checkpoints" in sqlite docs), so high reader contention can prevent the WAL
/// from ever getting compacted.
///
/// This seems to be generally present in high parallel uses of AI agents.
#[derive(Debug, Clone)]
pub(super) struct MaintenanceTask {
    _inner: MaintenanceTaskInner,
}

impl MaintenanceTask {
    pub(super) async fn spawn_active(
        opts: SqliteConnectOptions,
        info: EagerFutureCell<Info>,
    ) -> Self {
        match ActiveMaintenanceTask::connect(opts).await {
            Ok(conn) => Self {
                _inner: MaintenanceTaskInner::Active {
                    _compactor: ActiveMaintenanceTask::spawn(conn, info),
                },
            },
            Err(error) => {
                warn!(%error, "failed to open the WAL compactor connection; WAL compactor disabled");
                Self::inactive()
            }
        }
    }

    pub(super) fn inactive() -> Self {
        Self {
            _inner: MaintenanceTaskInner::Inactive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
    use sqlx::Connection;

    #[test]
    fn should_reclaim_requires_ratio_and_absolute_bytes() {
        // 1 GiB file, 30% free -> both thresholds tripped.
        let page_size = 4096;
        let total_pages = 262_144; // 1 GiB / 4 KiB
        let free_pages = 78_643; // ~30%
        assert!(ActiveMaintenanceTask::should_reclaim(total_pages, free_pages, page_size));
    }

    #[test]
    fn should_reclaim_false_when_ratio_below_threshold() {
        let page_size = 4096;
        let total_pages = 262_144;
        let free_pages = 13_107; // ~5% -> below 20%
        assert!(!ActiveMaintenanceTask::should_reclaim(total_pages, free_pages, page_size));
    }

    #[test]
    fn should_reclaim_false_when_absolute_bytes_below_floor() {
        // Tiny DB: 50% free but only ~1 MiB reclaimable -> below 8 MiB floor.
        let page_size = 4096;
        let total_pages = 512; // 2 MiB
        let free_pages = 256; // 50%, but 1 MiB
        assert!(!ActiveMaintenanceTask::should_reclaim(total_pages, free_pages, page_size));
    }

    #[test]
    fn should_reclaim_false_for_empty_db() {
        assert!(!ActiveMaintenanceTask::should_reclaim(0, 0, 4096));
    }

    async fn open_conn(path: &std::path::Path) -> SqliteConnection {
        let opts = SqliteConnectOptions::from_str(path.to_str().unwrap())
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        SqliteConnection::connect_with(&opts).await.unwrap()
    }

    #[tokio::test]
    async fn ensure_incremental_mode_converts_none_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let mut conn = open_conn(&path).await;

        // Simulate a legacy DB: default auto_vacuum=NONE with a populated table.
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, blob TEXT)")
            .execute(&mut conn)
            .await
            .unwrap();
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum").fetch_one(&mut conn).await.unwrap();
        assert_eq!(mode, 0, "precondition: legacy DB is auto_vacuum=NONE");

        let converted = ActiveMaintenanceTask::ensure_incremental_mode(&mut conn).await.unwrap();
        assert!(converted, "should report that a conversion happened");

        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum").fetch_one(&mut conn).await.unwrap();
        assert_eq!(mode, 2, "database should now be auto_vacuum=INCREMENTAL");

        // Idempotent: a second call is a no-op.
        let converted_again = ActiveMaintenanceTask::ensure_incremental_mode(&mut conn).await.unwrap();
        assert!(!converted_again, "second call should not re-convert");
    }

    #[tokio::test]
    async fn drain_freelist_empties_the_freelist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("churn.db");
        // Incremental mode must be set before tables exist for incremental_vacuum to work.
        let opts = SqliteConnectOptions::from_str(path.to_str().unwrap())
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("auto_vacuum", "INCREMENTAL");
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, blob BLOB)")
            .execute(&mut conn)
            .await
            .unwrap();

        // Allocate many pages, then free them to build a freelist.
        for i in 0..2000 {
            sqlx::query("INSERT INTO t (id, blob) VALUES (?, zeroblob(1024))")
                .bind(i)
                .execute(&mut conn)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM t").execute(&mut conn).await.unwrap();

        let before: i64 =
            sqlx::query_scalar("PRAGMA freelist_count").fetch_one(&mut conn).await.unwrap();
        assert!(before > 0, "precondition: freelist should be non-empty, got {before}");

        ActiveMaintenanceTask::drain_freelist(&mut conn).await.unwrap();

        let after: i64 =
            sqlx::query_scalar("PRAGMA freelist_count").fetch_one(&mut conn).await.unwrap();
        assert_eq!(after, 0, "freelist should be fully drained");
    }
}
