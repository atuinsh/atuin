//! Sqlite-related utilities.

mod builder;
mod compactor;
mod info;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

pub use builder::{Journaling, SqliteBuilder};
use compactor::Compactor;
pub use info::{Info, VersionError};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use thiserror::Error;

use crate::sync::EagerFutureCell;

/// An atuin-specific wrapper around Sqlite.
///
/// This has atuin-specific utilities.
#[derive(Debug, Clone)]
pub struct Sqlite {
    pool: SqlitePool,

    /// Sqlite has a limit on the total number of parameters you can bind in a single query.
    ///
    /// This value represents that.
    info: EagerFutureCell<Info>,

    /// A periodic task which compacts the WAL if necessary.
    compactor: Compactor,
}

#[derive(Debug, Error)]
pub enum SqliteOpenOrCreateError {
    #[error("the given path is a dangling symlink")]
    BadSymlink(PathBuf),

    #[error("failed to create directory for sqlite database: {0}")]
    FailedToCreateDir(std::io::Error),

    #[error("failed to parse connection options: {0}")]
    ConenctOptionsParsing(sqlx::Error),

    #[error("failed to create the sqlite pool")]
    PoolCreateError(sqlx::Error),

    #[error("failed to restrict permissions on the sqlite database: {0}")]
    FailedToSetPermissions(std::io::Error),
}

impl Sqlite {
    // TODO(markovejnovic): Modify this to accept the `SqliteDbUrl` type. The change is large at
    // this moment, since it would change settings contracts, etc, which I'd like to avoid for now.
    #[must_use]
    pub fn builder(uri: &OsStr) -> SqliteBuilder<'_> {
        SqliteBuilder::new(uri)
    }

    #[must_use]
    pub fn builder_in_memory() -> SqliteBuilder<'static> {
        SqliteBuilder::memory()
    }

    async fn connect(
        opts: SqliteConnectOptions,
        timeout: Duration,
    ) -> Result<Self, SqliteOpenOrCreateError> {
        let pool = SqlitePoolOptions::new()
            .acquire_timeout(timeout)
            .connect_with(opts)
            .await
            .map_err(SqliteOpenOrCreateError::PoolCreateError)?;

        Ok(Self {
            info: Info::new_eager_future(pool.clone()),
            pool,
            compactor: Compactor::inactive(),
        })
    }

    /// Get the pool of this structure.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get metadata on the database.
    #[must_use]
    pub async fn info(&self) -> Info {
        self.info.get().await
    }

    /// Close the underlying connection pool.
    #[cfg(feature = "test-utils")]
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
