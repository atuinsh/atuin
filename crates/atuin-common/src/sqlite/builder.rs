use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

use super::compactor::Compactor;
use super::{Sqlite, SqliteOpenOrCreateError};
use crate::path::PathExt;

/// Enum which controls what kind of WAL journaling mode is enabled.
///
/// Currently, this is an atuin-specific subset of [`SqliteJournalMode`].
#[derive(Debug, Clone, Copy)]
pub enum Journaling {
    Wal {
        /// The maximum size of the journal before sqlite is configured to automatically sweep it.
        ///
        /// Do note that this is a suggestion for Sqlite and under heavy concurrent reads will not
        /// be respected. See [`Compactor`] for a strict maximum size.
        #[allow(rustdoc::private_intra_doc_links)]
        max_size_hint: u64,
    },
    Delete,
}

pub struct SqliteBuilder<P> {
    path: P,
    timeout: Duration,
    journal: Option<Journaling>,
    synchronous: SqliteSynchronous,
    foreign_keys: bool,
    restrict_permissions: bool,
    regexp: bool,
}

impl<P: AsRef<Path>> SqliteBuilder<P> {
    /// When using the WAL, we set a journal limit in sqlite, which will cause sqlite to aim to have
    /// the WAL fit within that size.
    const DEFAULT_MAX_WAL_SIZE: u64 = 4 * 1024 * 1024;

    pub(super) fn new(path: P) -> Self {
        Self {
            path,
            timeout: Duration::from_secs(5),
            journal: Some(Journaling::Wal {
                max_size_hint: Self::DEFAULT_MAX_WAL_SIZE,
            }),
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
    pub fn journal(mut self, journal: Option<Journaling>) -> Self {
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
            .optimize_on_close(true, None)
            .synchronous(self.synchronous)
            .foreign_keys(self.foreign_keys)
            .create_if_missing(true);

        match self.journal {
            Some(Journaling::Wal { max_size_hint }) => {
                opts = opts
                    .journal_mode(SqliteJournalMode::Wal)
                    .pragma("journal_size_limit", max_size_hint.to_string())
            }
            Some(Journaling::Delete) => {
                opts = opts.journal_mode(SqliteJournalMode::Delete);
            }
            None => {
                opts = opts.journal_mode(SqliteJournalMode::Off);
            }
        };

        if self.regexp {
            opts = opts.with_regexp();
        }

        let mut sqlite = Sqlite::connect(opts.clone(), self.timeout).await?;

        if matches!(self.journal, Some(Journaling::Wal { .. })) {
            sqlite.compactor = Compactor::spawn_active(opts, sqlite.info.clone()).await;
        }

        #[cfg(unix)]
        if self.restrict_permissions && path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(SqliteOpenOrCreateError::FailedToSetPermissions)?;
        }

        Ok(sqlite)
    }
}
