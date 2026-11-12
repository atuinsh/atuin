use std::ffi::OsStr;
use std::io::Read as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

use super::compactor::Compactor;
use super::{Sqlite, SqliteOpenOrCreateError};
use crate::path::{AbsPathBuf, PathExt};

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

/// What a connection may do with the database it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read and write, creating the file and its directory if missing.
    ReadWrite,
    /// Never write, for a reader beside another process that owns the database.
    ///
    /// The file must exist. Its journal mode is left as its owner set it, and no WAL compactor or
    /// `PRAGMA optimize` runs. A WAL reader still needs the `-shm` file, which SQLite creates if
    /// the directory is writable.
    ReadOnly,
    /// [`Self::ReadWrite`], or read-only when the database exists on storage this process can't
    /// write. A read-only filesystem is such storage. Writes on that connection fail.
    PreferReadWrite,
}

/// How one attempt at opening the database reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    ReadWrite,
    /// Read through the WAL index beside the file, as a reader beside its writer does.
    ReadOnly,
    /// Read the main file alone, with no locks and no WAL index.
    ///
    /// This is only for a WAL database with no `-wal` or `-shm` file beside it, in a directory
    /// where neither can be created. SQLite ignores the WAL of an immutable database, so a database
    /// with one would read as it was before the WAL's transactions.
    Immutable,
}

pub struct SqliteBuilder<'a> {
    input: &'a OsStr,
    timeout: Duration,
    busy_timeout: Option<Duration>,
    journal: Option<Journaling>,
    synchronous: SqliteSynchronous,
    foreign_keys: bool,
    restrict_permissions: bool,
    regexp: bool,
    access: Access,
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
            busy_timeout: None,
            journal: Some(Journaling::Wal {
                max_size_hint: Self::DEFAULT_MAX_WAL_SIZE,
            }),
            synchronous: SqliteSynchronous::Normal,
            foreign_keys: true,
            restrict_permissions: false,
            regexp: false,
            access: Access::ReadWrite,
        }
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// How long a statement waits for another connection's lock before failing with "database is
    /// locked" (sqlite's busy timeout): sqlx's 5s when unset.
    #[must_use]
    pub fn busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = Some(timeout);
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
    pub fn access(mut self, access: Access) -> Self {
        self.access = access;
        self
    }

    #[must_use]
    pub fn regexp(mut self) -> Self {
        self.regexp = true;
        self
    }

    pub async fn open(self) -> Result<Sqlite, SqliteOpenOrCreateError> {
        match self.access {
            Access::ReadWrite => self.open_as(Mode::ReadWrite).await,
            Access::ReadOnly => self.open_as(Mode::ReadOnly).await,
            Access::PreferReadWrite => {
                let err = match self.open_as(Mode::ReadWrite).await {
                    Ok(sqlite) => return Ok(sqlite),
                    Err(err) => err,
                };
                let Some(mode) = self.database_file().and_then(|file| file.read_only_mode()) else {
                    return Err(err);
                };
                tracing::debug!(%err, ?mode, "the database's storage refuses writes; reading it");
                self.open_as(mode).await
            }
        }
    }

    /// The file this opens, with symlinks resolved, or `None` for an in-memory database, an input
    /// that isn't a path, or a file that doesn't exist.
    fn database_file(&self) -> Option<DatabaseFile> {
        if self.is_memory() {
            return None;
        }

        let opts = SqliteConnectOptions::from_str(self.input.to_str()?).ok()?;
        // SQLite follows symlinks, and keeps the WAL and its index beside the file they lead to.
        let path = AbsPathBuf::canonicalize(opts.get_filename()).ok()?;
        Some(DatabaseFile { path })
    }

    async fn open_as(&self, mode: Mode) -> Result<Sqlite, SqliteOpenOrCreateError> {
        let read_only = mode != Mode::ReadWrite;
        let path = self.input;

        let path_str = path.to_str().ok_or_else(|| {
            SqliteOpenOrCreateError::ConenctOptionsParsing(sqlx::Error::Configuration(
                format!("database path is not valid UTF-8: {path:?}").into(),
            ))
        })?;

        let mut opts = SqliteConnectOptions::from_str(path_str)
            .map_err(SqliteOpenOrCreateError::ConenctOptionsParsing)?
            .optimize_on_close(!read_only, None)
            .synchronous(self.synchronous)
            .foreign_keys(self.foreign_keys)
            .read_only(read_only)
            .immutable(mode == Mode::Immutable)
            .create_if_missing(!read_only);
        if let Some(busy) = self.busy_timeout {
            opts = opts.busy_timeout(busy);
        }

        match self.journal {
            // The journal mode is the owner's to set: changing it needs a write.
            _ if read_only => {}
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

            if !read_only
                && !fs_path.exists()
                && let Some(dir) = fs_path.parent()
            {
                std::fs::create_dir_all(dir).map_err(SqliteOpenOrCreateError::FailedToCreateDir)?;
            }
        }

        let mut sqlite = Sqlite::connect(opts.clone(), self.timeout).await?;

        if matches!(self.journal, Some(Journaling::Wal { .. })) && !is_memory && !read_only {
            sqlite.compactor = Compactor::spawn_active(opts, sqlite.info.clone()).await;
        }

        #[cfg(unix)]
        if self.restrict_permissions
            && !read_only
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

/// A database's main file on disk.
#[derive(Debug, Clone)]
struct DatabaseFile {
    path: AbsPathBuf,
}

impl DatabaseFile {
    /// How to read this database once opening it for writing failed.
    ///
    /// Returns `None` when its storage takes writes, since that failure was then something else.
    fn read_only_mode(&self) -> Option<Mode> {
        let dir_writable = self.path.parent()?.is_writable();
        if !self.path.exists() || (dir_writable && self.path.is_writable()) {
            return None;
        }

        // A reader of a WAL database needs a WAL index. With neither file beside the database,
        // SQLite would have to create one, which only a writable directory allows.
        Some(if self.is_wal() && !self.has_wal_files() && !dir_writable {
            Mode::Immutable
        } else {
            Mode::ReadOnly
        })
    }

    /// Whether this database is in WAL mode, as the read version in its header says.
    fn is_wal(&self) -> bool {
        // See https://www.sqlite.org/fileformat.html#file_format_version_numbers.
        const READ_VERSION_OFFSET: usize = 19;
        const WAL: u8 = 2;

        let mut header = [0; READ_VERSION_OFFSET + 1];
        std::fs::File::open(&self.path).and_then(|mut file| file.read_exact(&mut header)).is_ok()
            && header[READ_VERSION_OFFSET] == WAL
    }

    /// Whether a `-wal` or `-shm` file sits beside this database.
    fn has_wal_files(&self) -> bool {
        ["-wal", "-shm"].into_iter().any(|suffix| self.sidecar(suffix).exists())
    }

    /// The file SQLite keeps beside this database, named by `suffix` (`-wal`, `-shm`).
    fn sidecar(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        name.into()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::db;
    use crate::test_utils::ReadOnlyTree;

    /// How a database journals, and where its committed rows are when its storage stops taking
    /// writes.
    #[derive(Debug, Clone, Copy)]
    enum Journal {
        /// A WAL that still holds some rows, as a process that exits without closing the database
        /// leaves it.
        WalPending,
        /// A WAL checkpointed into the main file and gone, as a clean shutdown leaves it.
        WalCheckpointed,
        /// A rollback journal, which leaves every committed row in the main file.
        Rollback,
    }

    impl Journal {
        /// Copy a database holding three rows into `dir/data`, as this journal leaves it, and
        /// return the copy.
        async fn snapshot(self, dir: &TempDir) -> DatabaseFile {
            let written = DatabaseFile {
                path: AbsPathBuf::new(dir.path().join("written.db")).unwrap(),
            };
            let builder = Sqlite::builder(written.path.as_os_str());
            let builder = match self {
                Self::WalPending | Self::WalCheckpointed => builder,
                Self::Rollback => builder.journal(Some(Journaling::Delete)),
            };
            let writer = builder.open().await.unwrap();
            db::query("CREATE TABLE t (n INTEGER)").execute(writer.pool()).await.unwrap();
            db::query("INSERT INTO t VALUES (1), (2), (3)").execute(writer.pool()).await.unwrap();

            let suffixes: &[&str] = match self {
                Self::WalPending => {
                    assert!(std::fs::metadata(written.sidecar("-wal")).unwrap().len() > 0);
                    &["", "-wal", "-shm"]
                }
                Self::WalCheckpointed => {
                    db::query("PRAGMA wal_checkpoint(TRUNCATE)")
                        .execute(writer.pool())
                        .await
                        .unwrap();
                    &[""]
                }
                Self::Rollback => &[""],
            };

            let data = dir.path().join("data");
            std::fs::create_dir(&data).unwrap();
            let copy = DatabaseFile {
                path: AbsPathBuf::new(data.join("copy.db")).unwrap(),
            };
            for suffix in suffixes {
                std::fs::copy(written.sidecar(suffix), copy.sidecar(suffix)).unwrap();
            }
            copy
        }
    }

    /// How a test opens a snapshot on storage that refuses writes.
    #[derive(Debug, Clone, Copy)]
    enum Open {
        /// With [`Access::PreferReadWrite`], as a store does.
        Preferring,
        /// In the read-only mode that [`Access::PreferReadWrite`] falls back to. A locked directory
        /// lets through a read-write open that a read-only filesystem fails, so only this reaches
        /// that mode for every snapshot.
        FallbackMode,
    }

    /// How a test names the database it opens.
    #[derive(Debug, Clone, Copy)]
    enum Via {
        /// Its own path.
        File,
        /// A symlink in another directory. SQLite follows it, and keeps the WAL beside the file it
        /// leads to.
        Symlink,
    }

    impl Via {
        /// Name `file` this way, making any link under `dir`.
        fn path(self, dir: &TempDir, file: &DatabaseFile) -> PathBuf {
            match self {
                Self::File => file.path.to_path_buf(),
                Self::Symlink => {
                    let links = dir.path().join("links");
                    std::fs::create_dir(&links).unwrap();
                    let link = links.join("link.db");
                    std::os::unix::fs::symlink(&file.path, &link).unwrap();
                    link
                }
            }
        }
    }

    /// A database on storage that refuses writes reads every committed row, wherever the rows
    /// are, and refuses a write.
    #[rstest]
    #[case::wal_pending(Journal::WalPending, Mode::ReadOnly)]
    #[case::wal_checkpointed(Journal::WalCheckpointed, Mode::Immutable)]
    #[case::rollback(Journal::Rollback, Mode::ReadOnly)]
    #[tokio::test]
    async fn unwritable_storage_opens_read_only(
        #[case] journal: Journal,
        #[case] mode: Mode,
        #[values(Open::Preferring, Open::FallbackMode)] open: Open,
        #[values(Via::File, Via::Symlink)] via: Via,
    ) {
        const SQLITE_READONLY: u32 = 8;

        let dir = tempfile::tempdir().unwrap();
        let path = via.path(&dir, &journal.snapshot(&dir).await);
        let Some(_locked) = ReadOnlyTree::lock(dir.path()) else {
            return;
        };

        let builder = Sqlite::builder(path.as_os_str()).access(Access::PreferReadWrite);
        assert_eq!(builder.database_file().and_then(|db| db.read_only_mode()), Some(mode));
        let sqlite = match open {
            Open::Preferring => builder.open().await,
            Open::FallbackMode => builder.open_as(mode).await,
        }
        .unwrap();

        let rows: i64 =
            db::query_scalar("SELECT count(n) FROM t").fetch_one(sqlite.pool()).await.unwrap();
        assert_eq!(rows, 3);

        let err = db::query("INSERT INTO t VALUES (4)")
            .execute(sqlite.pool())
            .await
            .expect_err("a read-only database took a write");
        let code = err.as_database_error().and_then(|e| e.code()).expect("a database error");
        assert_eq!(code.parse::<u32>().unwrap() & 0xff, SQLITE_READONLY, "{err}");
    }

    /// Storage that takes writes has no read-only mode, so a failure to open it for writing, such
    /// as a lock held too long, stays an error.
    #[rstest]
    #[tokio::test]
    async fn writable_storage_has_no_read_only_mode(
        #[values(Journal::WalPending, Journal::WalCheckpointed, Journal::Rollback)]
        journal: Journal,
    ) {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(journal.snapshot(&dir).await.read_only_mode(), None);
    }
}
