use std::ffi::{CStr, c_int};
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;
use std::sync::Arc;
use std::time::Duration;

use sqlx::Sqlite;
use sqlx::error::DatabaseError;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{LockedSqliteHandle, SqlitePool};
use thiserror::Error;
use tracing::warn;

use crate::futures::Backoff;
use crate::sync::EagerFutureCell;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("failed to parse the sqlite version: {0}")]
    Parsing(#[from] semver::Error),
    #[error("failed to query the sqlite version: {0}")]
    Query(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Error)]
pub enum SqlitePathError {
    #[error("the path reported by sqlite is NULL.")]
    NullPath,

    #[error("the path reported by sqlite is not a utf8 path")]
    NonUtf8Path(#[from] Utf8Error),

    #[error("failed to acquire a connection to query the sqlite path: {0}")]
    Acquire(#[from] Arc<sqlx::Error>),
}

/// Metadata which is queried on startup, and never again.
#[derive(Debug, Clone)]
pub struct Info {
    // Info returned by FFI calls.
    ffi_info: FfiInfo,

    /// The version of the currently active database.
    ///
    /// Note that the error is behind an Arc. Annoyingly, [`VersionError`] cannot be [`Clone`], so
    /// we have to wrap it in an [`Arc`]. It's the cold path anyways.
    pub version: Result<semver::Version, Arc<VersionError>>,
}

/// Data which can only be queried through raw FFI cals.
#[derive(Debug, Clone)]
struct FfiInfo {
    variable_number_limit: Option<usize>,
    wal_path: Result<PathBuf, SqlitePathError>,
}

impl FfiInfo {
    async fn query(pool: &SqlitePool) -> Self {
        // Connections can potentially fail. Under some operations (migrations, for example), sqlite
        // can return SQLITE_BUSY or SQLITE_LOCKED. In effect, this means the whole database is
        // locked by someone else and the lock will be removed soon-ish.
        //
        // `Self::acquire_retrying` will perform that retry logic.
        let mut conn = match Self::acquire_retrying(pool).await {
            Ok(conn) => conn,
            Err(err) => return Self::unavailable(err),
        };

        let mut handle = match conn.lock_handle().await {
            Ok(handle) => handle,
            Err(err) => return Self::unavailable(err),
        };

        let vnl = Self::query_variable_number_limit(&mut handle);
        let wal_path = Self::query_wal_path(&mut handle);

        drop(handle);

        Self {
            variable_number_limit: vnl,
            wal_path,
        }
    }

    fn unavailable(err: sqlx::Error) -> Self {
        Self {
            variable_number_limit: None,
            wal_path: Err(SqlitePathError::Acquire(Arc::new(err))),
        }
    }

    fn err_is_locked(err: &sqlx::Error) -> bool {
        err.as_database_error()
            .and_then(DatabaseError::code)
            .and_then(|code| code.parse::<c_int>().ok())
            .is_some_and(|code| {
                let primary = code & 0xff;
                primary == libsqlite3_sys::SQLITE_BUSY || primary == libsqlite3_sys::SQLITE_LOCKED
            })
    }

    async fn acquire_retrying(pool: &SqlitePool) -> Result<PoolConnection<Sqlite>, sqlx::Error> {
        const TIMEOUT: Duration = Duration::from_millis(500);
        const BACKOFF: Backoff = Backoff::Exponential {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(200),
            factor: NonZeroU32::new(2).unwrap(),
        };

        BACKOFF
            .retry(
                || async move {
                    match pool.acquire().await {
                        Err(err) if Self::err_is_locked(&err) => ControlFlow::Continue(err),
                        result => ControlFlow::Break(result),
                    }
                },
                TIMEOUT,
            )
            .await?
    }

    fn query_variable_number_limit(handle: &mut LockedSqliteHandle<'_>) -> Option<usize> {
        let raw_handle = handle.as_raw_handle();

        #[allow(unsafe_code, reason = "FFI call to read SQLITE_LIMIT_VARIABLE_NUMBER")]
        let limit = unsafe {
            libsqlite3_sys::sqlite3_limit(
                raw_handle.as_ptr(),
                libsqlite3_sys::SQLITE_LIMIT_VARIABLE_NUMBER,
                -1,
            )
        };

        match usize::try_from(limit) {
            Ok(l) => Some(l),
            Err(err) => {
                warn!(
                    "failed to convert {limit} to a number to compute bind param count: {err}. \
                     performance could be degraded."
                );
                None
            }
        }
    }

    fn query_wal_path(handle: &mut LockedSqliteHandle<'_>) -> Result<PathBuf, SqlitePathError> {
        #[allow(unsafe_code)]
        let db_filename = unsafe {
            libsqlite3_sys::sqlite3_db_filename(handle.as_raw_handle().as_ptr(), c"main".as_ptr())
        };

        if db_filename.is_null() {
            return Err(SqlitePathError::NullPath);
        }

        // Worth noting here that you have to be careful -- sqlite docs explicitly specify that the
        // path you give this function **must** be the return pointer coming from
        // `sqlite3_db_filename`.
        #[allow(unsafe_code)]
        let wal_filename = unsafe { libsqlite3_sys::sqlite3_filename_wal(db_filename) };

        if wal_filename.is_null() {
            return Err(SqlitePathError::NullPath);
        }

        #[allow(unsafe_code)]
        let file_cstr = unsafe { CStr::from_ptr(wal_filename).to_bytes() };

        Ok(PathBuf::from(std::str::from_utf8(file_cstr)?))
    }
}

impl Info {
    /// Old versions of sqlite supported up to 999 params.
    ///
    /// This is used as a fallback in case our query fails.
    const MAX_BIND_PARAMS_FALLBACK: usize = 999;

    /// # Panics
    ///
    /// Panics if there is no active [`tokio::runtime::Handle`].
    #[must_use]
    pub fn new_eager_future(pool: SqlitePool) -> EagerFutureCell<Self> {
        EagerFutureCell::new(
            async move {
                // Please note that `query_variable_number_limit` will take a lock on the database
                // pool, meaning that all connections will have to wait. We definitely want to do
                // that last and parallelize the rest.

                // First we do the things that do not need the whole connection lock.
                let version = Self::query_version(&pool).await.map_err(Arc::new);

                // Finally, we do the things that need that nasty lock.
                let ffi_info = FfiInfo::query(&pool).await;

                Self { ffi_info, version }
            },
            &tokio::runtime::Handle::current(),
        )
    }

    async fn query_version(pool: &SqlitePool) -> Result<semver::Version, VersionError> {
        let str: String =
            crate::db::query_scalar("SELECT sqlite_version()").fetch_one(pool).await?;
        Ok(semver::Version::parse(&str)?)
    }

    /// Get the maximum number of `?` binds a SQL query can have.
    #[must_use]
    pub fn variable_number_limit(&self) -> usize {
        self.ffi_info.variable_number_limit.unwrap_or(Self::MAX_BIND_PARAMS_FALLBACK)
    }

    /// Get the path to the WAL database.
    pub fn wal_path(&self) -> Result<&Path, SqlitePathError> {
        self.ffi_info.wal_path.as_deref().map_err(Clone::clone)
    }
}
