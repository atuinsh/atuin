//! Sqlite-related utilities.

mod builder;
mod compactor;
pub mod fts;
mod info;
pub mod observe;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

pub use builder::{Journaling, SqliteBuilder};
use compactor::Compactor;
pub use info::{Info, VersionError};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use thiserror::Error;

use crate::sync::EagerFutureCell;

/// SQLite (extended) result codes that indicate transient lock contention and
/// are therefore worth retrying, as opposed to a terminal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientResultCode {
    Busy,
    Locked,
    BusyRecovery,
    LockedSharedCache,
    BusySnapshot,
    LockedVtab,
    BusyTimeout,
}

impl TransientResultCode {
    /// Classify a SQLite result code (as `sqlx` reports it, i.e. the decimal extended code) into a
    /// transient variant, or `None` if it is not one.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "5" => Self::Busy,
            "6" => Self::Locked,
            "261" => Self::BusyRecovery,
            "262" => Self::LockedSharedCache,
            "517" => Self::BusySnapshot,
            "518" => Self::LockedVtab,
            "773" => Self::BusyTimeout,
            _ => return None,
        })
    }

    /// Classify the result code carried by `e`, or `None` if `e` is not a transient database error.
    #[must_use]
    pub fn from_error(e: &sqlx::Error) -> Option<Self> {
        Self::from_code(&e.as_database_error()?.code()?)
    }
}

/// Quotes `ident` as an SQLite identifier, doubling any `` ` `` inside it.
#[must_use]
pub fn quote_ident(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

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

    #[error("failed to create the sqlite pool: {0}")]
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use sqlx::AssertSqlSafe;

    use super::{Sqlite, quote_ident};
    use crate::db::{query, query_scalar};

    #[fixture]
    async fn sqlite() -> Sqlite {
        Sqlite::builder_in_memory().open().await.unwrap()
    }

    #[rstest]
    #[case::backtick("a`b")]
    #[case::double_quote("a\"b")]
    #[case::statement_breakout("x`; DROP TABLE t; --")]
    #[tokio::test]
    async fn quote_ident_round_trips(#[future(awt)] sqlite: Sqlite, #[case] name: &str) {
        let sql = format!("CREATE TABLE t ({} INTEGER)", quote_ident(name));
        query::<sqlx::Sqlite>(AssertSqlSafe(sql)).execute(sqlite.pool()).await.unwrap();

        let columns: Vec<String> =
            query_scalar::<sqlx::Sqlite, String>("SELECT name FROM pragma_table_info('t')")
                .fetch_all(sqlite.pool())
                .await
                .unwrap();
        assert_eq!(columns, [name]);
    }

    #[rstest]
    #[tokio::test]
    async fn quote_ident_misspelled_column_is_an_error(#[future(awt)] sqlite: Sqlite) {
        query::<sqlx::Sqlite>("CREATE TABLE t (id INTEGER)").execute(sqlite.pool()).await.unwrap();

        let sql = format!("SELECT {} FROM t", quote_ident("idd"));
        let err =
            query::<sqlx::Sqlite>(AssertSqlSafe(sql)).execute(sqlite.pool()).await.unwrap_err();
        assert!(err.to_string().contains("no such column"), "{err}");
    }
}
