use std::sync::Arc;

use sqlx::sqlite::SqlitePool;
use thiserror::Error;
use tracing::warn;

use crate::sync::EagerFutureCell;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("failed to parse the sqlite version: {0}")]
    Parsing(#[from] semver::Error),
    #[error("failed to query the sqlite version: {0}")]
    Query(#[from] sqlx::Error),
}

/// Metadata which is queried on startup, and never again.
#[derive(Debug, Clone)]
pub struct Info {
    /// The best-effort estimate of the maximum parameters that this sqlite can bind to.
    pub variable_number_limit: usize,

    /// The version of the currently active database.
    ///
    /// Note that the error is behind an Arc. Annoyingly, [`VersionError`] cannot be [`Clone`], so
    /// we have to wrap it in an [`Arc`]. It's the cold path anyways.
    pub version: Result<semver::Version, Arc<VersionError>>,
}

impl Info {
    /// Old versions of sqlite supported up to 999 params.
    ///
    /// This is used as a fallback in case our query fails.
    const MAX_BIND_PARAMS_FALLBACK: usize = 999;

    /// # Panics
    ///
    /// Panics if there is no active [`tokio::runtime::Handle`].
    pub fn new_eager_future(pool: SqlitePool) -> EagerFutureCell<Self> {
        EagerFutureCell::new(
            async move {
                // Please note that `query_variable_number_limit` will take a lock on the database
                // pool, meaning that all connections will have to wait. We definitely want to do
                // that last and parallelize the rest.

                // First we do the things that do not need the whole connection lock.
                let version = Self::query_version(&pool).await.map_err(Arc::new);

                // Finally, we do the things that need that nasty lock.
                let variable_number_limit = Self::query_variable_number_limit(&pool).await;

                Self {
                    variable_number_limit,
                    version,
                }
            },
            &tokio::runtime::Handle::current(),
        )
    }

    async fn query_version(pool: &SqlitePool) -> Result<semver::Version, VersionError> {
        let str: String = sqlx::query_scalar("SELECT sqlite_version()").fetch_one(pool).await?;
        Ok(semver::Version::parse(&str)?)
    }

    /// Queries the database for the maximum number of bind parameters.
    async fn query_variable_number_limit(pool: &SqlitePool) -> usize {
        let mut conn = match pool.acquire().await {
            Ok(c) => c,
            Err(err) => {
                warn!(
                    "failed to grab a connection to query bind param count: {err}. performance \
                     could be degraded."
                );
                return Self::MAX_BIND_PARAMS_FALLBACK;
            }
        };

        let mut handle = match conn.lock_handle().await {
            Ok(h) => h,
            Err(err) => {
                warn!(
                    "failed to lock the connection to query bind param count: {err}. performance \
                     could be degraded."
                );
                return Self::MAX_BIND_PARAMS_FALLBACK;
            }
        };

        let raw_handle = handle.as_raw_handle();

        #[allow(unsafe_code, reason = "FFI call to read SQLITE_LIMIT_VARIABLE_NUMBER")]
        let limit = unsafe {
            libsqlite3_sys::sqlite3_limit(
                raw_handle.as_ptr(),
                libsqlite3_sys::SQLITE_LIMIT_VARIABLE_NUMBER,
                -1,
            )
        };

        drop(handle);

        match usize::try_from(limit) {
            Ok(l) => l,
            Err(err) => {
                warn!(
                    "failed to convert {limit} to a number to compute bind param count: {err}. \
                     performance could be degraded."
                );
                Self::MAX_BIND_PARAMS_FALLBACK
            }
        }
    }
}
