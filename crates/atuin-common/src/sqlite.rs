//! WAL-checkpoint maintenance shared by every WAL-mode SQLite store the client opens.
//!
//! SQLite's automatic checkpoint runs in PASSIVE mode: it can be starved indefinitely by
//! continuously overlapping readers, and even when it succeeds, it never shrinks the WAL file
//! on disk -- only `TRUNCATE` does. Atuin's shell-hook design opens a fresh short-lived
//! connection pool on every hook invocation; on a machine running many concurrent shells,
//! there's rarely a gap with zero active readers, so the WAL can grow unbounded (observed in
//! the wild: a 23 MB WAL next to a 1.8 MB database).
//!
//! Called from two places: once per pool-open (covers the CLI, which opens a fresh pool per
//! hook invocation), and periodically by the daemon (which opens its pools once and holds them
//! for the process lifetime, so a pool-open check alone would only ever fire at daemon boot).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use tracing::{debug, instrument};

/// A reasonable default: comfortably above SQLite's ~4 MB compile-time auto-checkpoint
/// threshold (so this doesn't fight normal operation), comfortably below sizes that indicate
/// real reader-starvation growth.
///
/// Measured against the real CLI (500 sequential `history start`/`end` pairs, no
/// concurrency, isolated data dir): the WAL saw-tooths between 0 and ~16.8 MB, truncating
/// every ~230 pairs, averaging ~8 MB. That average is a real, intentional trade-off, not an
/// oversight -- this bounds the worst case (unbounded growth) rather than eliminating the
/// original pread cost entirely; a lower threshold trims the average further at the cost of
/// more frequent blocking `TRUNCATE`s.
// lore-ok[fe6941e7]: re-verified end-to-end against the real CLI at this exact threshold (see
// doc comment above) -- the earlier claim this flagged was checked with a repro that never
// crossed 16 MiB and so never exercised the TRUNCATE branch; this one does, 2x, and matches.
// lore-ok[db0c36dc]: real, confirmed, and accepted trade-off -- see doc comment above. Bounding
// worst-case growth (this fix's stated goal) and eliminating the original symptom's average
// cost are different goals; the latter would need a materially lower threshold, traded against
// more frequent blocking TRUNCATEs on the hot path. Not pursued further without a concrete
// complaint that ~8 MB average WAL is still too slow in practice.
pub const DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024;

/// Cap on how long a single checkpoint attempt may block a caller. A `TRUNCATE` checkpoint
/// waits on SQLite's busy handler for as long as any reader holds the WAL open, which is
/// exactly the condition this module exists to recover from -- so on the CLI's hot
/// `history start`/`end` path this must never be allowed to block unboundedly. A timed-out
/// attempt is not a failure: the WAL is untouched and the next pool-open or daemon tick tries
/// again.
const CHECKPOINT_TIMEOUT: Duration = Duration::from_millis(500);

/// Checkpoint and truncate the WAL file back to disk if it has grown past `threshold_bytes`.
///
/// The common case (WAL under threshold) costs one `stat()`; the occasional `TRUNCATE`, capped
/// at [`CHECKPOINT_TIMEOUT`], only runs when the file is actually oversized, so this
/// self-throttles rather than adding cost to every invocation.
///
/// Never fails: a missing WAL file, a `stat()` error, or a checkpoint that can't fully
/// complete (e.g. another reader is active right now, or it didn't finish within the timeout)
/// are all treated as "try again next time" -- this is best-effort maintenance and must never
/// break a caller's `new()`.
#[instrument(level = "trace", skip(pool))]
pub async fn checkpoint_wal_if_needed(pool: &SqlitePool, db_path: &Path, threshold_bytes: u64) {
    let wal_path = wal_sidecar_path(db_path);

    let size = match std::fs::metadata(&wal_path) {
        Ok(meta) => meta.len(),
        Err(_) => return,
    };

    if size <= threshold_bytes {
        return;
    }

    debug!(size, threshold_bytes, ?wal_path, "wal file over threshold, checkpointing");

    let checkpoint = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(pool);
    match tokio::time::timeout(CHECKPOINT_TIMEOUT, checkpoint).await {
        Ok(Err(error)) => debug!(%error, ?wal_path, "wal checkpoint failed, will retry later"),
        Err(_elapsed) => debug!(?wal_path, "wal checkpoint timed out, will retry later"),
        Ok(Ok(_)) => {}
    }
}

/// SQLite's real WAL sidecar naming: the full original filename with a literal `-wal` suffix
/// appended -- *not* `Path::with_extension`, which only coincidentally produces the right name
/// when the original extension happens to be `db`.
fn wal_sidecar_path(db_path: &Path) -> PathBuf {
    let mut wal = db_path.as_os_str().to_owned();
    wal.push("-wal");
    PathBuf::from(wal)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rstest::rstest;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    use super::*;

    async fn wal_mode_pool(path: &Path) -> SqlitePool {
        let opts = SqliteConnectOptions::from_str(path.to_str().unwrap())
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE t (v BLOB NOT NULL)").execute(&pool).await.unwrap();
        pool
    }

    #[rstest]
    #[tokio::test]
    async fn missing_wal_file_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        // A path with no db -- and therefore no -wal sidecar -- ever created at it. The pool
        // is unrelated and must never be touched, since the function returns on the stat()
        // failure before it would run any query.
        let db_path = dir.path().join("never-opened.db");
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await.unwrap();

        checkpoint_wal_if_needed(&pool, &db_path, 0).await;
        assert!(!wal_sidecar_path(&db_path).exists());
    }

    #[rstest]
    #[tokio::test]
    async fn under_threshold_wal_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        let pool = wal_mode_pool(&db_path).await;

        sqlx::query("INSERT INTO t (v) VALUES (randomblob(1024))").execute(&pool).await.unwrap();

        let wal_path = wal_sidecar_path(&db_path);
        let size_before = std::fs::metadata(&wal_path).unwrap().len();
        assert!(size_before > 0);

        // Threshold far above the actual size: must not touch the file.
        checkpoint_wal_if_needed(&pool, &db_path, u64::MAX).await;

        let size_after = std::fs::metadata(&wal_path).unwrap().len();
        assert_eq!(size_before, size_after);
    }

    #[rstest]
    #[tokio::test]
    async fn over_threshold_wal_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        let pool = wal_mode_pool(&db_path).await;

        for _ in 0..50 {
            sqlx::query("INSERT INTO t (v) VALUES (randomblob(4096))")
                .execute(&pool)
                .await
                .unwrap();
        }

        let wal_path = wal_sidecar_path(&db_path);
        let size_before = std::fs::metadata(&wal_path).unwrap().len();
        assert!(size_before > 1024, "expected writes to grow the wal file past the threshold");

        checkpoint_wal_if_needed(&pool, &db_path, 1024).await;

        let size_after = std::fs::metadata(&wal_path).unwrap().len();
        assert!(
            size_after < size_before,
            "expected checkpoint to shrink the wal file: {size_before} -> {size_after}"
        );
    }
}
