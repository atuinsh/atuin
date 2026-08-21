//! Sqlite-related utilities.

mod builder;
mod info;
mod table;

use std::path::PathBuf;
use std::time::Duration;

pub use builder::{SqliteBuilder, SqliteBuilderRoot};
pub use info::{Info, VersionError};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
pub use table::{Col, ColKind, Conflict, KeyBind, Schema, Table, TableView};
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

    #[error("failed to run sqlite migrations: {0}")]
    Migrate(sqlx::migrate::MigrateError),
}

impl Sqlite {
    #[must_use]
    pub fn builder() -> SqliteBuilderRoot {
        SqliteBuilderRoot
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
            info: Info::eager_future(pool.clone()),
            pool,
        })
    }

    /// Get the pool of this structure.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get metadata on the database.
    #[must_use]
    pub async fn info(&self) -> &Info {
        self.info.get().await
    }

    /// Close the underlying connection pool.
    #[cfg(feature = "test-utils")]
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
