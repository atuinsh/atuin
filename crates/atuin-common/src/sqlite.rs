//! Shared SQLite pool setup for Atuin's SQLite-backed stores.
//!
//! Compiled only when the `sqlite` feature is enabled. Use [`pool`] to build a
//! configured [`SqlitePool`]; the defaults match the common Atuin store and the
//! builder methods cover the per-store divergences.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::utils;

/// Builder for a configured [`SqlitePool`], created via [`pool`].
///
/// Defaults: WAL journal, `optimize_on_close`, `create_if_missing`, SQLite's
/// default synchronous mode and foreign-key setting, no regexp, no permission
/// restriction. Adjust with the builder methods, then call [`open`](Self::open).
#[derive(Debug, Clone)]
pub struct PoolBuilder<'a> {
    path: &'a Path,
    timeout: f64,
    journal_mode: SqliteJournalMode,
    synchronous: Option<SqliteSynchronous>,
    foreign_keys: bool,
    regexp: bool,
    restrict_permissions: bool,
}

/// Begin configuring a [`SqlitePool`] for the database at `path`, acquiring
/// connections within `timeout` seconds.
pub fn pool(path: &Path, timeout: f64) -> PoolBuilder<'_> {
    PoolBuilder {
        path,
        timeout,
        journal_mode: SqliteJournalMode::Wal,
        synchronous: None,
        foreign_keys: false,
        regexp: false,
        restrict_permissions: false,
    }
}

impl<'a> PoolBuilder<'a> {
    /// Journal mode (default [`SqliteJournalMode::Wal`]).
    #[must_use]
    pub fn journal_mode(mut self, mode: SqliteJournalMode) -> Self {
        self.journal_mode = mode;
        self
    }

    /// Synchronous mode. When left unset, SQLite's default (FULL) applies.
    #[must_use]
    pub fn synchronous(mut self, synchronous: SqliteSynchronous) -> Self {
        self.synchronous = Some(synchronous);
        self
    }

    /// Explicitly enforce foreign keys. When left `false`, the setter is not
    /// called and SQLite's default (foreign keys ON in sqlx) applies.
    #[must_use]
    pub fn foreign_keys(mut self, enabled: bool) -> Self {
        self.foreign_keys = enabled;
        self
    }

    /// Register the `REGEXP` operator (needs sqlx's `regexp` feature, which the
    /// `sqlite` feature enables).
    #[must_use]
    pub fn regexp(mut self, enabled: bool) -> Self {
        self.regexp = enabled;
        self
    }

    /// After opening, restrict the database file to `0o600` on Unix. No-op on
    /// non-Unix and for in-memory databases.
    #[must_use]
    pub fn restrict_permissions(mut self, enabled: bool) -> Self {
        self.restrict_permissions = enabled;
        self
    }

    /// Prepare the parent directory, apply the configured pragmas, connect the
    /// pool, and (if requested) restrict file permissions.
    ///
    /// Errors if the path is not valid UTF-8 or is a broken symlink. In-memory
    /// databases (paths containing `:memory:`) skip all filesystem preparation.
    pub async fn open(self) -> sqlx::Result<SqlitePool> {
        let path = self.path;

        let path_str = path.as_os_str().to_str().ok_or_else(|| {
            sqlx::Error::Configuration(
                format!("sqlite db path is not valid UTF-8: {path:?}").into(),
            )
        })?;
        let is_memory = path_str.contains(":memory:");

        if !is_memory {
            if utils::broken_symlink(path) {
                return Err(sqlx::Error::Configuration(
                    format!(
                        "sqlite db path ({path:?}) is a broken symlink; unable to read or create replacement"
                    )
                    .into(),
                ));
            }

            if !path.exists()
                && let Some(dir) = path.parent()
            {
                std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
            }
        }

        let mut opts = SqliteConnectOptions::from_str(path_str)?
            .journal_mode(self.journal_mode)
            .optimize_on_close(true, None)
            .create_if_missing(true);

        if let Some(synchronous) = self.synchronous {
            opts = opts.synchronous(synchronous);
        }
        if self.foreign_keys {
            opts = opts.foreign_keys(true);
        }
        if self.regexp {
            opts = opts.with_regexp();
        }

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(self.timeout))
            .connect_with(opts)
            .await?;

        #[cfg(unix)]
        if self.restrict_permissions && !is_memory {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(sqlx::Error::Io)?;
        }

        Ok(pool)
    }
}

/// Query the underlying SQLite library version (`SELECT sqlite_version()`).
pub async fn version(pool: &SqlitePool) -> sqlx::Result<String> {
    sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::{pool, version};
    use sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
    use std::path::Path;

    #[tokio::test]
    async fn opens_in_memory_and_runs_a_query() {
        let p = pool(Path::new(":memory:"), 5.0).open().await.unwrap();
        let one: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&p).await.unwrap();
        assert_eq!(one, 1);
    }

    #[tokio::test]
    async fn version_is_non_empty() {
        let p = pool(Path::new(":memory:"), 5.0).open().await.unwrap();
        assert!(!version(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn creates_file_and_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("test.db");
        let _p = pool(&path, 5.0).open().await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn regexp_enabled_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .regexp(true)
            .open()
            .await
            .unwrap();
        let m: i64 = sqlx::query_scalar("SELECT 'abc' REGEXP 'b'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(m, 1);
    }

    #[tokio::test]
    async fn foreign_keys_on_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .foreign_keys(true)
            .open()
            .await
            .unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn synchronous_normal_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .synchronous(SqliteSynchronous::Normal)
            .open()
            .await
            .unwrap();
        let s: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(s, 1); // 1 == NORMAL
    }

    #[tokio::test]
    async fn journal_mode_delete_when_requested() {
        // Journal mode is observable only on a real file (in-memory reports
        // "memory"), so use a temp file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.db");
        let p = pool(&path, 5.0)
            .journal_mode(SqliteJournalMode::Delete)
            .open()
            .await
            .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "delete");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_symlink_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("does-not-exist.db");
        let link = dir.path().join("link.db");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(pool(&link, 5.0).open().await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restrict_permissions_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perms.db");
        let _p = pool(&path, 5.0)
            .restrict_permissions(true)
            .open()
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
