//! Utility which compacts a SQLite database.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use thiserror::Error;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::db::sqlite::Info;
use crate::sync::EagerFutureCell;

#[derive(Debug, Error)]
enum WalCompactionError {
    #[error("failed to compact the WAL due to a sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("failed to compact the WAL due to an IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct ActiveCompactor {
    _task: Arc<AbortOnDropHandle<()>>,
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

    #[must_use]
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

        let mut ticker = tokio::time::interval(Self::PERIOD);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = Self::compact_wal(&mut conn, &wal).await {
                warn!(%error, "failed to compact the WAL");
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
    ) -> Result<(), WalCompactionError> {
        let meta = wal.metadata().await?;
        if meta.len() < Self::THRESHOLD_BYTES {
            return Ok(());
        }

        let (busy, log, checkpointed): (i64, i64, i64) =
            crate::db::query_as("PRAGMA wal_checkpoint(PASSIVE)").fetch_one(&mut *conn).await?;

        if busy != 0 || checkpointed < log {
            // This query risks causing reader starvation, but this is the intent.
            crate::db::query("PRAGMA wal_checkpoint(RESTART)").execute(conn).await?;
        }

        Ok(())
    }

    async fn connect(opts: SqliteConnectOptions) -> Result<SqliteConnection, WalCompactionError> {
        let conn = SqliteConnection::connect_with(&opts.busy_timeout(Self::MAX_TIMEOUT)).await?;

        Ok(conn)
    }
}

#[derive(Debug, Clone)]
enum CompactorInner {
    Active {
        _compactor: ActiveCompactor,
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
pub(super) struct Compactor {
    _inner: CompactorInner,
}

impl Compactor {
    pub(super) async fn spawn_active(
        opts: SqliteConnectOptions,
        info: EagerFutureCell<Info>,
    ) -> Self {
        match ActiveCompactor::connect(opts).await {
            Ok(conn) => Self {
                _inner: CompactorInner::Active {
                    _compactor: ActiveCompactor::spawn(conn, info),
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
            _inner: CompactorInner::Inactive,
        }
    }
}
