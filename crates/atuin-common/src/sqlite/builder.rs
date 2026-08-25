use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

use super::{Sqlite, SqliteOpenOrCreateError};
use crate::path::PathExt;

pub struct SqliteBuilder<P> {
    path: P,
    timeout: Duration,
    journal: SqliteJournalMode,
    synchronous: SqliteSynchronous,
    foreign_keys: bool,
    restrict_permissions: bool,
    regexp: bool,
}

impl<P: AsRef<Path>> SqliteBuilder<P> {
    pub(super) fn new(path: P) -> Self {
        Self {
            path,
            timeout: Duration::from_secs(5),
            journal: SqliteJournalMode::Wal,
            synchronous: SqliteSynchronous::Normal,
            foreign_keys: true,
            restrict_permissions: false,
            regexp: false,
        }
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn journal(mut self, journal: SqliteJournalMode) -> Self {
        self.journal = journal;
        self
    }

    #[must_use]
    pub fn synchronous(mut self, synchronous: SqliteSynchronous) -> Self {
        self.synchronous = synchronous;
        self
    }

    #[must_use]
    pub fn foreign_keys(mut self, foreign_keys: bool) -> Self {
        self.foreign_keys = foreign_keys;
        self
    }

    #[must_use]
    pub fn restrict_permissions(mut self) -> Self {
        self.restrict_permissions = true;
        self
    }

    #[must_use]
    pub fn regexp(mut self) -> Self {
        self.regexp = true;
        self
    }

    pub async fn open(self) -> Result<Sqlite, SqliteOpenOrCreateError> {
        let path = self.path.as_ref();

        if path.is_dangling_symlink() {
            return Err(SqliteOpenOrCreateError::BadSymlink(path.to_path_buf()));
        }

        if !path.exists()
            && let Some(dir) = path.parent()
        {
            std::fs::create_dir_all(dir).map_err(SqliteOpenOrCreateError::FailedToCreateDir)?;
        }

        let path_str = path.to_str().ok_or_else(|| {
            SqliteOpenOrCreateError::ConenctOptionsParsing(sqlx::Error::Configuration(
                format!("database path is not valid UTF-8: {path:?}").into(),
            ))
        })?;

        let mut opts = SqliteConnectOptions::from_str(path_str)
            .map_err(SqliteOpenOrCreateError::ConenctOptionsParsing)?
            .journal_mode(self.journal)
            .optimize_on_close(true, None)
            .synchronous(self.synchronous)
            .foreign_keys(self.foreign_keys)
            .create_if_missing(true);

        if self.regexp {
            opts = opts.with_regexp();
        }

        let sqlite = Sqlite::connect(opts, self.timeout).await?;

        #[cfg(unix)]
        if self.restrict_permissions && path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(SqliteOpenOrCreateError::FailedToSetPermissions)?;
        }

        Ok(sqlite)
    }
}
