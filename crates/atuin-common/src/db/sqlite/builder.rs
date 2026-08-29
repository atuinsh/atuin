use std::ffi::OsStr;
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

pub struct SqliteBuilder<'a> {
    input: &'a OsStr,
    timeout: Duration,
    journal: Option<Journaling>,
    synchronous: SqliteSynchronous,
    foreign_keys: bool,
    restrict_permissions: bool,
    regexp: bool,
}

impl<'a> SqliteBuilder<'a> {
    /// When using the WAL, we set a journal limit in sqlite, which will cause sqlite to aim to have
    /// the WAL fit within that size.
    const DEFAULT_MAX_WAL_SIZE: u64 = 4 * 1024 * 1024;

    pub(super) fn new(input: &'a OsStr) -> Self {
        Self::with_input(input)
    }

    pub(super) fn memory() -> Self {
        Self::with_input(OsStr::new(":memory:"))
    }

    #[must_use]
    pub fn is_memory(&self) -> bool {
        let path = self.input;
        let Some(raw) = path.to_str() else {
            return false;
        };

        let stripped = raw
            .strip_prefix("sqlite://")
            .or_else(|| raw.strip_prefix("sqlite:"))
            .or_else(|| raw.strip_prefix("file://"))
            .or_else(|| raw.strip_prefix("file:"))
            .unwrap_or(raw);

        let (database, params) = match stripped.split_once('?') {
            Some((database, params)) => (database, Some(params)),
            None => (stripped, None),
        };

        database == ":memory:"
            || params.is_some_and(|params| params.split('&').any(|pair| pair == "mode=memory"))
    }

    fn with_input(input: &'a OsStr) -> Self {
        Self {
            input,
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
        let path = self.input;

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
                    .pragma("journal_size_limit", max_size_hint.to_string());
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

        let is_memory = self.is_memory();
        let on_disk = (!is_memory).then(|| opts.get_filename().to_path_buf());

        if let Some(fs_path) = &on_disk {
            if fs_path.is_dangling_symlink() {
                return Err(SqliteOpenOrCreateError::BadSymlink(fs_path.clone()));
            }

            if !fs_path.exists()
                && let Some(dir) = fs_path.parent()
            {
                std::fs::create_dir_all(dir).map_err(SqliteOpenOrCreateError::FailedToCreateDir)?;
            }
        }

        let mut sqlite = Sqlite::connect(opts.clone(), self.timeout).await?;

        if matches!(self.journal, Some(Journaling::Wal { .. })) && !is_memory {
            sqlite.compactor = Compactor::spawn_active(opts, sqlite.info.clone()).await;
        }

        #[cfg(unix)]
        if self.restrict_permissions
            && let Some(fs_path) = &on_disk
            && fs_path.exists()
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(fs_path, std::fs::Permissions::from_mode(0o600))
                .map_err(SqliteOpenOrCreateError::FailedToSetPermissions)?;
        }

        Ok(sqlite)
    }
}
