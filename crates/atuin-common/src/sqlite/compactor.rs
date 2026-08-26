//! Utility which compacts a SQLite database.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use thiserror::Error;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::sqlite::Info;
use crate::sync::EagerFutureCell;

#[derive(Debug, Error)]
enum WalCompactionError {
    #[error("failed to compact the WAL due to a sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("failed to compact the WAL due to an IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
struct ActiveCompactor {
    task: AbortOnDropHandle<()>,
}

impl ActiveCompactor {
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

    async fn new(
        opts: SqliteConnectOptions,
        wal_path: impl AsRef<Path>,
    ) -> Result<Self, WalCompactionError> {
        let conn = SqliteConnection::connect_with(&opts.busy_timeout(Self::MAX_TIMEOUT)).await?;
        let wal = tokio::fs::File::open(wal_path).await?;

        Ok(Self::spawn(conn, wal))
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
    ) -> Result<(), WalCompactionError> {
        let meta = wal.metadata().await?;
        if meta.len() < Self::THRESHOLD_BYTES {
            return Ok(());
        }

        // This query risks causing reader starvation, but this is the intent. In order
        sqlx::query("PRAGMA wal_checkpoint(RESTART)").execute(conn).await?;

        Ok(())
    }

    fn spawn(mut conn: SqliteConnection, wal: tokio::fs::File) -> Self {
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Self::PERIOD);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = Self::compact_wal(&mut conn, &wal).await {
                    warn!(%error, "failed to compact the WAL");
                }
            }
        });

        Self {
            task: AbortOnDropHandle::new(task),
        }
    }
}

enum CompactorInner {
    Active(ActiveCompactor),
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
pub(super) struct Compactor {
    inner: CompactorInner,
}

impl Compactor {
    pub fn spawn_active(info: EagerFutureCell<Info>) -> Self {}

    pub fn inactive() -> Self {
        Self {
            inner: CompactorInner::Inactive,
        }
    }
}
