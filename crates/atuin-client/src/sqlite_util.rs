use sqlx::sqlite::SqlitePool;
use tracing::instrument;

/// Query the live SQLite library for `SQLITE_LIMIT_VARIABLE_NUMBER`: the maximum
/// number of bound parameters allowed in a single prepared statement.
///
/// Statements that bind a variable number of values must chunk their input against
/// this so a single statement never exceeds it. We bundle SQLite (via `sqlx`'s
/// `sqlite` feature), so in practice this is the modern default of 32766, but
/// querying it honours any build-time override.
#[instrument(level = "trace", skip_all, err)]
pub(crate) async fn max_bind_params(pool: &SqlitePool) -> sqlx::Result<usize> {
    let mut conn = pool.acquire().await?;
    let mut handle = conn.lock_handle().await?;
    let raw = handle.as_raw_handle();

    // SAFETY: `raw` is a valid, live `sqlite3` handle for as long as `handle` is
    // held. Passing a negative `newVal` queries the limit without changing it.
    #[allow(unsafe_code, reason = "FFI call to read SQLITE_LIMIT_VARIABLE_NUMBER")]
    let limit = unsafe {
        libsqlite3_sys::sqlite3_limit(
            raw.as_ptr(),
            libsqlite3_sys::SQLITE_LIMIT_VARIABLE_NUMBER,
            -1,
        )
    };

    usize::try_from(limit).ok().filter(|&n| n > 0).ok_or_else(|| {
        sqlx::Error::Protocol(format!("sqlite reported an invalid variable limit: {limit}"))
    })
}
