use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

use super::{Sqlite, SqliteOpenOrCreateError};
use crate::path::PathExt;

enum SqliteLocation {
    File(PathBuf),
    Memory,
}

pub struct SqliteBuilderRoot;

#[allow(clippy::unused_self)]
impl SqliteBuilderRoot {
    #[must_use]
    pub fn file(self, path: impl AsRef<Path>) -> SqliteBuilder {
        let path = path.as_ref();
        // `:memory:` (and the `sqlite::memory:` URI form) is SQLite's in-memory sentinel, not a real
        // filename; route it through the memory path so pooled connections share one database (a
        // filename would give each connection its own empty db, or worse, a real on-disk file).
        if matches!(path.to_str(), Some(":memory:" | "sqlite::memory:")) {
            return SqliteBuilder::new(SqliteLocation::Memory);
        }
        SqliteBuilder::new(SqliteLocation::File(path.to_path_buf()))
    }

    #[must_use]
    pub fn memory(self) -> SqliteBuilder {
        SqliteBuilder::new(SqliteLocation::Memory)
    }
}

pub struct SqliteBuilder {
    location: SqliteLocation,
    timeout: Duration,
    journal: SqliteJournalMode,
    synchronous: SqliteSynchronous,
    foreign_keys: bool,
    restrict_permissions: bool,
    regexp: bool,
}

impl SqliteBuilder {
    fn new(location: SqliteLocation) -> Self {
        Self {
            location,
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
        let opts = match &self.location {
            SqliteLocation::File(path) => {
                if path.is_dangling_symlink() {
                    return Err(SqliteOpenOrCreateError::BadSymlink(path.clone()));
                }

                if !path.exists()
                    && let Some(dir) = path.parent()
                {
                    std::fs::create_dir_all(dir)
                        .map_err(SqliteOpenOrCreateError::FailedToCreateDir)?;
                }

                SqliteConnectOptions::new().filename(path)
            }
            SqliteLocation::Memory => SqliteConnectOptions::from_str(":memory:")
                .map_err(SqliteOpenOrCreateError::ConenctOptionsParsing)?,
        };

        let mut opts = opts
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
        if self.restrict_permissions
            && let SqliteLocation::File(path) = &self.location
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(SqliteOpenOrCreateError::FailedToSetPermissions)?;
        }

        Ok(sqlite)
    }
}
