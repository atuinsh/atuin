//! Utilities for managing the daemon's pidfile.
//!
//! The Atuin daemon creates a pidfile and holds a lock on the pidfile while it's running; this
//! prevents two daemons from running at the same time.
//!
//! Some information about the daemon is stored in the pidfile itself:
//!
//! - The daemon's PID
//! - The daemon's version
//! - The daemon's socket path
//!
//! The format of the pidfile is as follows:
//!
//! ```text
//! <pid>
//! v1
//! <JSON-encoded PidfileInfo>
//! ```
//!
//! This format is backward-compatible with the old format (PID on first line, version on second
//! line). The version always starts with a number (comes from `CARGO_PKG_VERSION`); so the "v" in
//! "v1" distinguishes the new format from the old. Additionally, the version line was not actually
//! used for anything, so it will not impact older clients to have something else there.
//!
//! To ensure that clients don't read the pidfile in a half-written state, there is an additional
//! file, the *pidfile lock*, which is held whenever the pidfile is being written to or read from.
//! It has the same path as the pidfile but with `.lock` appended. Unlike the pidfile itself, which
//! is locked for long periods of time by the daemon, the pidfile lock should be held for the
//! minimum amount of time necessary -- only when reading from or writing to the pidfile.

use std::fmt::{self, Display};
use std::fs::File;
use std::io::{self, Seek, Write};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::fs::lock::{LockError, LockMode, LockOptions};
use atuin_common::string::AutoOsString;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::service::startup_lock::StartupLock;

const FORMAT_V1: &str = "v1";

/// Structured data stored in the pidfile after the `v1` marker.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidfileInfo {
    #[serde(skip)]
    /// The daemon's PID.
    pub pid: u32,

    /// The daemon's version ([`crate::VERSION`]).
    pub version: String,

    /// The path to the socket the daemon is listening on.
    ///
    /// This is [`None`] on Windows, or if `daemon.systemd_socket` is true in `config.toml`.
    #[serde_as(as = "Option<AutoOsString>")]
    pub socket_path: Option<PathBuf>,
}

impl PidfileInfo {
    /// Create a [`PidfileInfo`] for the current process (assuming this process is the daemon).
    #[must_use]
    pub fn current(settings: &DaemonSettings) -> Self {
        #[cfg(not(unix))]
        let _ = settings;

        Self {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            #[cfg(unix)]
            socket_path: (!settings.systemd_socket).then(|| settings.socket_path().into_owned()),
            #[cfg(not(unix))]
            socket_path: None,
        }
    }

    /// Parse the contents of a pidfile, returning `None` if it's not in the `v1` format.
    fn parse(contents: &str) -> Option<Self> {
        let (pid, rest) = contents.split_once('\n')?;
        let pid: u32 = pid
            .parse()
            .inspect_err(|e| tracing::debug!("failed to parse {pid:?} as pid in pidfile: {e}"))
            .ok()?;

        let (format, payload) = rest.split_once('\n')?;
        if format != FORMAT_V1 {
            return None;
        }

        let mut info: Self = serde_json::from_str(payload)
            .inspect_err(|e| tracing::debug!("could not parse daemon pidfile payload: {e}"))
            .ok()?;
        info.pid = pid;
        Some(info)
    }
}

/// An error that occurred while trying to [acquire] a [`PidfileGuard`].
///
/// [acquire]: Pidfile::acquire
#[derive(Debug, thiserror::Error)]
pub enum AcquireGuardError {
    #[error(transparent)]
    Lock(#[from] LockPidfileError),
    #[error(transparent)]
    Write(#[from] WritePidfileError),
}

/// An error that occurred while trying to lock the pidfile.
///
/// See [`AcquireGuardError::Lock`].
#[derive(Debug, thiserror::Error)]
pub struct LockPidfileError {
    pub path: PathBuf,
    pub inner: LockError,
}

impl Display for LockPidfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
            LockError::WouldBlock => {
                write!(f, "daemon already running (pidfile locked at {})", self.path.display())
            }
            _ => write!(
                f,
                "could not lock daemon pidfile at {}: {}",
                self.path.display(),
                self.inner
            ),
        }
    }
}

/// An error that occurred while writing to the pidfile.
///
/// See [`AcquireGuardError::Write`].
#[derive(Debug, thiserror::Error)]
pub enum WritePidfileError {
    #[error("could not acquire daemon pidfile lock at {}: {inner}", .path.display())]
    Lock {
        path: PathBuf,
        inner: LockError,
    },

    #[error("could not write daemon pidfile {}: {inner}", .path.display())]
    Write {
        path: PathBuf,
        inner: io::Error,
    },

    #[error("could not serialize pidfile data: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Controls ownership of the daemon pidfile.
///
/// The lock is held until this type is dropped.
pub struct PidfileGuard {
    // Closing the file releases the lock.
    _file: File,
}

/// The pidfile the service holds locked for as long as it runs.
#[derive(Debug)]
pub struct Pidfile {
    path: PathBuf,
}

/// Errors from waiting for the service to release its pidfile.
#[derive(Debug, thiserror::Error)]
pub enum PidfileWaitError {
    #[error("a daemon still holds its pidfile at {} after {}ms", .path.display(), .timeout.as_millis())]
    StillHeld {
        path: PathBuf,
        timeout: Duration,
    },
    #[error("failed to lock daemon pidfile at {}: {lock}", .path.display())]
    Lock {
        path: PathBuf,
        lock: LockError,
    },
}

impl Pidfile {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Lock the pidfile and write `info`, the running daemon's, into it. The daemon holds the
    /// returned guard for as long as it runs.
    ///
    /// TODO(taylordotfish): On Windows, this locks the entire byte range of the pidfile, which
    /// means that while the lock is held, no other process can read the contents of the pidfile.
    /// This means that [`ActiveService::evict`](crate::service::ActiveService::evict) can never
    /// successfully kill a running daemon, because either:
    ///
    /// * The daemon is running, which means the lock is held, which means the client can't read the
    ///   daemon's PID from the pidfile; or
    /// * The daemon isn't running, so the client can read the pidfile, but the PID in the pidfile
    ///   necessarily isn't running.
    ///
    /// This is not a critical issue -- it only affects the `evict` path in which somethis
    /// has already gone wrong -- but it would still be good to fix. A potential approach would be
    /// to use `windows-sys` to lock a single byte well past the actual contents of the file. SQLite
    /// uses this approach; its locks start at 0x40000000.
    pub fn acquire(&self, info: &PidfileInfo) -> Result<PidfileGuard, AcquireGuardError> {
        let mut file = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .try_open(&self.path)
        .map_err(|e| LockPidfileError {
            path: self.path.clone(),
            inner: e,
        })?;

        self.write(&mut file, info)?;

        Ok(PidfileGuard { _file: file })
    }

    /// Replace the contents of the pidfile, open as `file`, with `info`.
    fn write(&self, file: &mut File, info: &PidfileInfo) -> Result<(), WritePidfileError> {
        let json = serde_json::to_string(info)?;
        let lock_path = self.lock_path();

        let _lock = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .open(&lock_path)
        .map_err(|e| WritePidfileError::Lock {
            path: lock_path,
            inner: e,
        })?;

        file.set_len(0)
            .and_then(|()| {
                // `set_len` doesn't move the cursor; we need to do that separately.
                file.rewind()?;
                writeln!(file, "{}\n{FORMAT_V1}\n{json}", info.pid)?;
                file.flush()
            })
            .map_err(|e| WritePidfileError::Write {
                path: self.path.clone(),
                inner: e,
            })
    }

    /// Read the [`PidfileInfo`] the daemon wrote, if it wrote one in the `v1` format.
    #[must_use]
    pub fn info(&self) -> Option<PidfileInfo> {
        let options = LockOptions {
            // If the pidfile lock doesn't exist, we return early (no pidfile lock means there's no
            // daemon running or the daemon is an older version that doesn't use the v1 format), so
            // there's no point in creating the pidfile lock.
            create: false,
            mode: LockMode::Shared,
        };

        let lock_path = self.lock_path();
        let lock = match options.open(&lock_path) {
            Ok(lock) => lock,
            Err(e) => {
                if !e.is_not_found() {
                    tracing::debug!(
                        "could not lock daemon pidfile lock at {}: {e}",
                        lock_path.display()
                    );
                }
                return None;
            }
        };

        let contents = match std::fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::debug!(
                        "could not read daemon pidfile at {}: {e}",
                        self.path.display()
                    );
                }
                return None;
            }
        };
        drop(lock);

        PidfileInfo::parse(&contents)
    }

    /// Read the daemon's PID, in the v1 format or the old one.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        let lock_path = self.lock_path();

        let _lock = LockOptions {
            create: true,
            mode: LockMode::Shared,
        }
        .open(&lock_path)
        .inspect_err(|e| {
            tracing::debug!("could not lock pidfile lock at {}: {e}", lock_path.display());
        })
        .ok()?;

        let contents = std::fs::read_to_string(&self.path).ok()?;
        let pid = contents.lines().next()?.trim();
        pid.parse()
            .inspect_err(|e| {
                tracing::debug!(
                    "failed to parse {pid:?} as pid in pidfile at {}: {e}",
                    self.path.display()
                );
            })
            .ok()
    }

    /// Whether a running daemon holds the pidfile.
    #[must_use]
    pub fn is_live(&self) -> bool {
        LockOptions {
            create: false,
            // The daemon always acquires an exclusive lock on the pidfile. Here, try to obtain a
            // shared lock to slightly reduce the chance of false positives in case some other
            // process is holding a lock on the file.
            mode: LockMode::Shared,
        }
        .try_open(&self.path)
        .is_err_and(|e| matches!(e, LockError::WouldBlock))
    }

    /// The path to the lock used to guard reads and writes of the pidfile itself.
    fn lock_path(&self) -> PathBuf {
        let mut os = self.path.as_os_str().to_os_string();
        os.push(".lock");
        PathBuf::from(os)
    }

    /// The lock beside the pidfile that a client holds while it replaces or starts the service,
    /// waited on for up to `timeout`.
    pub(super) fn startup_lock(&self, timeout: Duration) -> StartupLock {
        StartupLock::new(self.startup_lock_path(), timeout)
    }

    fn startup_lock_path(&self) -> PathBuf {
        let mut os = self.path.as_os_str().to_os_string();
        os.push(".startup.lock");
        PathBuf::from(os)
    }

    /// Wait until no service holds the pidfile.
    pub(super) async fn released(&self, timeout: Duration) -> Result<(), PidfileWaitError> {
        let exclusive = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        };

        // Taking the lock is only how to learn that it is free; it is released again straight away.
        match exclusive.wait(&self.path, timeout).await {
            Ok(_released) => Ok(()),
            Err(LockError::Timeout) => Err(PidfileWaitError::StillHeld {
                path: self.path.clone(),
                timeout,
            }),
            Err(lock) => Err(PidfileWaitError::Lock {
                path: self.path.clone(),
                lock,
            }),
        }
    }
}

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub enum SocketPathError {
    /// The socket is in use by another program.
    ///
    /// This error can be returned when `ATUIN_HOME` is set. Atuin started storing the socket path
    /// in the pidfile before `ATUIN_HOME` was introduced, so if there's no socket path in the
    /// pidfile but the socket already exists, it's an indication that the socket is in use by
    /// another program (potentially another Atuin daemon running with a different value of
    /// `ATUIN_HOME`).
    #[error("{}", Self::display_in_use(.path, *.user_defined))]
    InUse {
        /// The path to the socket.
        path: PathBuf,
        /// Whether the path came from `daemon.socket_path` in config.toml.
        user_defined: bool,
    },
}

#[cfg(unix)]
impl SocketPathError {
    /// Helper for displaying [`Self::InUse`].
    fn display_in_use(path: &Path, user_defined: bool) -> impl std::fmt::Display + use<'_> {
        std::fmt::from_fn(move |f| {
            writeln!(f, "daemon socket '{}' is in use by another program", path.display())?;
            let config_path = atuin_common::dirs::config_path("config.toml");
            if user_defined {
                write!(
                    f,
                    "please ensure `daemon.socket_path` has a unique value in {}",
                    config_path.display(),
                )
            } else {
                write!(
                    f,
                    "please set an explicit `daemon.socket_path` in {}",
                    config_path.display()
                )
            }
        })
    }
}

#[cfg(unix)]
impl Pidfile {
    /// The path to the daemon's socket.
    ///
    /// If the daemon is running and has recorded its socket path in the pidfile, this function returns
    /// that. Otherwise:
    ///
    /// * If `ATUIN_HOME` is unset (the default), [`daemon.existing_socket_path()`][0] is returned,
    ///   which will check for existing sockets in legacy locations.
    ///
    /// * If `ATUIN_HOME` is set, [`daemon.preferred_socket_path()`][1] is returned, which does
    ///   not check for legacy sockets, as Atuin stopped using them before `ATUIN_HOME` was introduced.
    ///   In addition, an error is returned if that socket already exists: Atuin started storing the
    ///   socket path in the pidfile before `ATUIN_HOME`, so if an existing socket isn't in our pidfile,
    ///   it almost certainly isn't ours. This behavior is intended to reduce the chance of the client
    ///   connecting to the wrong daemon.
    ///
    /// [0]: atuin_client::settings::Daemon::existing_socket_path
    /// [1]: atuin_client::settings::Daemon::preferred_socket_path
    ///
    /// TODO(markovejnovic): Eventually get to the point where we don't need to take the settings.
    pub fn socket_path(&self, daemon: &DaemonSettings) -> Result<PathBuf, SocketPathError> {
        let existing_socket_path = || daemon.existing_socket_path().into_owned();

        let socket_path = self.info().and_then(|info| info.socket_path);

        if !atuin_common::dirs::atuin_home_is_set() {
            return Ok(socket_path.unwrap_or_else(existing_socket_path));
        }

        // If `ATUIN_HOME` is set, require the pidfile to be live to avoid connecting to the wrong
        // daemon. This should potentially be done even when `ATUIN_HOME` isn't set, but that would be a
        // larger and possibly riskier change. For now, scope the check to when `ATUIN_HOME` is set,
        // which is when we would most expect there to be multiple daemons running on the user's system.
        if let Some(path) = socket_path.filter(|_| self.is_live()) {
            return Ok(path);
        }

        // If there's no socket path in the pidfile (or the pidfile was missing/stale) and `ATUIN_HOME`
        // is set, don't look for or connect to existing sockets. Atuin switched to storing the socket
        // path in the pidfile before `ATUIN_HOME` was introduced, so an existing socket is almost
        // certainly that of another program, potentially another daemon running with a different value
        // of `ATUIN_HOME`, which is conceptually a separate profile that we should not connect to.
        let path = daemon.preferred_socket_path().into_owned();

        // Avoid connecting to an existing, in-use socket. We match on `Ok(true)` rather than
        // `Ok(true) | Err(_)` because an error doesn't necessarily mean the socket is in use (e.g.,
        // `EACCES`), and many such errors will be encountered again later in the pipeline and handled
        // better there.
        if path.exists() && matches!(atuin_common::os::unix::socket_in_use(&path), Ok(true)) {
            return Err(SocketPathError::InUse {
                path,
                user_defined: daemon.socket_path.is_some(),
            });
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::path::Path;

    #[cfg(unix)]
    use atuin_client::settings::Settings;
    #[cfg(unix)]
    use atuin_common::env::MockEnv;
    use atuin_common::path::AbsPathBuf;
    #[cfg(unix)]
    use atuin_common::path::PathBufExt as _;
    #[cfg(unix)]
    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::*;

    struct Pidfile {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    impl Pidfile {
        fn create(&self) -> File {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&self.path)
                .unwrap()
        }
    }

    #[fixture]
    fn pidfile() -> Pidfile {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atuin-daemon.pid");
        Pidfile { _dir: dir, path }
    }

    /// Info for this process with the given socket path.
    fn info_with(socket_path: Option<&str>) -> PidfileInfo {
        PidfileInfo {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            socket_path: socket_path.map(PathBuf::from),
        }
    }

    fn info() -> PidfileInfo {
        info_with(Some("/tmp/atuin-1000/atuin.sock"))
    }

    fn daemon_settings(pidfile: &Path, systemd_socket: bool) -> DaemonSettings {
        DaemonSettings {
            enabled: false,
            autostart: false,
            sync_frequency: None,
            pidfile_path: AbsPathBuf::new(pidfile.to_owned()).unwrap(),
            socket_path: Some(AbsPathBuf::new(pidfile.with_file_name("atuin.sock")).unwrap()),
            systemd_socket,
            tcp_port: 8889,
        }
    }

    #[rstest]
    fn test_guard_acquire_and_drop(pidfile: Pidfile, #[values(false, true)] systemd_socket: bool) {
        let settings = daemon_settings(&pidfile.path, systemd_socket);
        {
            let _guard = acquire(&settings).unwrap();
            // Guard holds an exclusive lock — on Windows other handles cannot
            // read the file, so we verify contents after the guard is dropped.
        }

        let contents = std::fs::read_to_string(&pidfile.path).unwrap();
        assert_eq!(contents.lines().next().unwrap(), std::process::id().to_string());
        let info = at(&pidfile).info().unwrap();
        assert_eq!(info.version, crate::VERSION);
        // The socket path is only recorded if the daemon chose it: not under systemd, which passes
        // the daemon a socket, and not on Windows, which doesn't use one.
        let expected =
            (cfg!(unix) && !systemd_socket).then(|| pidfile.path.with_file_name("atuin.sock"));
        assert_eq!(info.socket_path, expected);

        // After guard is dropped, lock should be released — acquiring again must succeed.
        let _guard2 = acquire(&settings).unwrap();
    }

    #[rstest]
    fn test_guard_prevents_double_acquire(pidfile: Pidfile) {
        let settings = daemon_settings(&pidfile.path, false);
        let _guard = acquire(&settings).unwrap();
        let result = acquire(&settings);
        assert!(matches!(
            result,
            Err(AcquireGuardError::Lock(LockPidfileError {
                inner: LockError::WouldBlock,
                ..
            }))
        ));
    }

    #[rstest]
    fn test_lock_path() {
        assert_eq!(
            super::Pidfile {
                path: PathBuf::from("/tmp/atuin-daemon.pid")
            }
            .lock_path(),
            PathBuf::from("/tmp/atuin-daemon.pid.lock")
        );
    }

    #[rstest]
    fn test_write_format(pidfile: Pidfile) {
        at(&pidfile).write(&mut pidfile.create(), &info()).unwrap();

        let contents = std::fs::read_to_string(&pidfile.path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "got: {contents:?}");
        assert_eq!(lines[0], std::process::id().to_string());
        assert_eq!(lines[1], FORMAT_V1);
        assert!(lines[2].contains(r#""/tmp/atuin-1000/atuin.sock""#), "got: {contents:?}");
        assert!(at(&pidfile).lock_path().exists());
    }

    #[rstest]
    fn test_write_replaces_longer_contents(pidfile: Pidfile) {
        let mut file = pidfile.create();
        let long = info_with(Some("/a/much/longer/path/than/the/next/one.sock"));
        at(&pidfile).write(&mut file, &long).unwrap();
        at(&pidfile).write(&mut file, &info()).unwrap();

        assert_eq!(at(&pidfile).info(), Some(info()));
    }

    #[rstest]
    #[case::some(Some("/tmp/atuin-1000/atuin.sock"))]
    #[case::none(None)]
    fn test_round_trip(pidfile: Pidfile, #[case] socket_path: Option<&str>) {
        let info = info_with(socket_path);
        at(&pidfile).write(&mut pidfile.create(), &info).unwrap();
        assert_eq!(at(&pidfile).info(), Some(info));
    }

    #[rstest]
    fn test_read_without_lock_file(pidfile: Pidfile) {
        at(&pidfile).write(&mut pidfile.create(), &info()).unwrap();
        std::fs::remove_file(at(&pidfile).lock_path()).unwrap();

        assert_eq!(at(&pidfile).info(), None);
        assert!(!at(&pidfile).lock_path().exists(), "read must not create the lock");
    }

    #[rstest]
    fn test_read_without_pidfile(pidfile: Pidfile) {
        File::create(at(&pidfile).lock_path()).unwrap();
        assert_eq!(at(&pidfile).info(), None);
    }

    #[rstest]
    #[case::legacy("1234\n18.9.0\n")]
    #[case::pid_only("1234\n")]
    #[case::half_written("1234\nv1\n{\"version\":\"18.9")]
    #[case::bad_json("1234\nv1\nnot json\n")]
    #[case::empty("")]
    fn test_read_unusable(pidfile: Pidfile, #[case] contents: &str) {
        File::create(at(&pidfile).lock_path()).unwrap();
        std::fs::write(&pidfile.path, contents).unwrap();
        assert_eq!(at(&pidfile).info(), None);
    }

    #[rstest]
    fn test_read_ignores_unknown_fields(pidfile: Pidfile) {
        File::create(at(&pidfile).lock_path()).unwrap();
        std::fs::write(
            &pidfile.path,
            "1234\nv1\n{\"version\":\"1.2.3\",\"socket_path\":null,\"future\":true}\n",
        )
        .unwrap();
        assert_eq!(
            at(&pidfile).info(),
            Some(PidfileInfo {
                pid: 1234,
                version: "1.2.3".into(),
                socket_path: None
            })
        );
    }

    #[cfg(unix)]
    fn path_buf() -> impl Strategy<Value = PathBuf> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        proptest::collection::vec(any::<u8>(), 0..64).prop_map(|b| OsString::from_vec(b).into())
    }

    #[cfg(unix)]
    proptest! {
        #[test]
        fn prop_parse_round_trip(
            pid in any::<u32>(),
            version in ".*",
            socket_path in proptest::option::of(path_buf()),
        ) {
            let info = PidfileInfo { pid, version, socket_path };
            let contents = format!(
                "{pid}\n{FORMAT_V1}\n{}\n",
                serde_json::to_string(&info).unwrap()
            );
            prop_assert_eq!(PidfileInfo::parse(&contents), Some(info));
        }
    }

    /// The pidfile at `pidfile`'s path.
    fn at(pidfile: &Pidfile) -> super::Pidfile {
        super::Pidfile {
            path: pidfile.path.clone(),
        }
    }

    #[rstest]
    #[case("/tmp/atuin-daemon.pid", "/tmp/atuin-daemon.pid.startup.lock")]
    #[case("/path/to/pidfile", "/path/to/pidfile.startup.lock")]
    fn startup_lock_sits_beside_the_pidfile(#[case] pidfile: &str, #[case] expected: &str) {
        let pidfile = super::Pidfile {
            path: PathBuf::from(pidfile),
        };
        assert_eq!(pidfile.startup_lock_path(), PathBuf::from(expected));
    }

    /// Lock the pidfile `daemon` configures, as a daemon started with it does.
    fn acquire(daemon: &DaemonSettings) -> Result<PidfileGuard, AcquireGuardError> {
        super::Pidfile::new(&daemon.pidfile_path).acquire(&PidfileInfo::current(daemon))
    }

    /// Where the pidfile `daemon` configures says the daemon listens.
    #[cfg(unix)]
    fn socket_path(daemon: &DaemonSettings) -> Result<PathBuf, SocketPathError> {
        super::Pidfile::new(&daemon.pidfile_path).socket_path(daemon)
    }

    /// Settings for a client whose daemon socket is configured at `socket_path`.
    #[cfg(unix)]
    fn settings(pidfile: &Path, socket_path: &Path, systemd_socket: bool) -> Settings {
        let mut settings = Settings::default();
        settings.daemon.pidfile_path = AbsPathBuf::new(pidfile.to_owned()).unwrap();
        settings.daemon.socket_path = Some(AbsPathBuf::new(socket_path.to_owned()).unwrap());
        settings.daemon.systemd_socket = systemd_socket;
        settings
    }

    /// A daemon configured for `old.sock` has exited, leaving its pidfile behind, and the client is
    /// now configured for `new.sock`.
    #[cfg(unix)]
    #[rstest]
    #[case::pidfile_path_is_used(false, false, "old.sock")]
    #[case::systemd_daemon_records_no_path(true, false, "new.sock")]
    #[case::both_systemd(true, true, "new.sock")]
    fn test_socket_path_with_stale_pidfile(
        #[case] daemon_systemd_socket: bool,
        #[case] client_systemd_socket: bool,
        #[case] expected: &str,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), false);

        let pidfile = dir.path().join("atuin-daemon.pid");

        let old = settings(&pidfile, &dir.path().join("old.sock"), daemon_systemd_socket);
        drop(acquire(&old.daemon).unwrap());

        let new = settings(&pidfile, &dir.path().join("new.sock"), client_systemd_socket);
        assert_eq!(socket_path(&new.daemon).unwrap(), dir.path().join(expected));
    }

    #[cfg(unix)]
    #[rstest]
    fn test_socket_path_without_pidfile(#[values(false, true)] systemd_socket: bool) {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(
            &dir.path().join("atuin-daemon.pid"),
            &dir.path().join("a.sock"),
            systemd_socket,
        );
        assert_eq!(socket_path(&settings.daemon).unwrap(), dir.path().join("a.sock"));
    }

    /// A mock environment with `HOME` and `TMPDIR` in `dir`, and `ATUIN_HOME` too if
    /// `atuin_home_set`, so that neither the settings nor the default socket path involve the
    /// real home or `/tmp`.
    #[cfg(unix)]
    fn env(dir: &Path, atuin_home_set: bool) -> MockEnv {
        let env = MockEnv::install();
        env.set("HOME", dir.join("home"));
        env.set("TMPDIR", dir.join("tmp"));
        if atuin_home_set {
            env.set("ATUIN_HOME", dir.join("profile"));
        }
        env
    }

    /// Listen on a Unix socket at `path`, as a running daemon does, until the listener is
    /// dropped. Dropping it leaves the socket file behind with nothing listening, as a daemon that
    /// was killed does.
    #[cfg(unix)]
    fn listen(path: &Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::net::UnixListener::bind(path).unwrap()
    }

    /// The default socket path for `dir` as [`env`] sets it up, or the configured one.
    #[cfg(unix)]
    fn preferred_path(dir: &Path, configured: bool) -> PathBuf {
        if configured {
            dir.join("configured.sock")
        } else {
            dir.join("tmp")
                .joined(format!("atuin-{}", atuin_common::os::unix::uid()))
                .joined("atuin.sock")
        }
    }

    /// Settings with the socket at [`preferred_path`], and a pidfile in `dir`.
    #[cfg(unix)]
    fn settings_in(dir: &Path, configured: bool) -> Settings {
        let mut settings =
            settings(&dir.join("atuin-daemon.pid"), &dir.join("configured.sock"), false);
        if !configured {
            settings.daemon.socket_path = None;
        }
        assert_eq!(settings.daemon.preferred_socket_path(), preferred_path(dir, configured));
        settings
    }

    /// With `ATUIN_HOME` set, a live socket that the pidfile doesn't name belongs to something
    /// else, such as a daemon for another `ATUIN_HOME`, so the client refuses it rather than
    /// connect. Without `ATUIN_HOME`, an existing socket is connected to as before.
    #[cfg(unix)]
    #[rstest]
    fn an_unrecorded_live_socket_is_refused_only_with_atuin_home(
        #[values(false, true)] atuin_home_set: bool,
        #[values(false, true)] configured: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), atuin_home_set);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        // Nothing's there yet, so it's free for this profile's daemon. (Without `ATUIN_HOME`, the
        // client would also fall back to sockets elsewhere, like the real `/tmp/atuin-$UID`.)
        if atuin_home_set {
            assert_eq!(socket_path(&settings.daemon).unwrap(), path);
        }

        let _listener = listen(&path);
        let result = socket_path(&settings.daemon);
        if !atuin_home_set {
            assert_eq!(result.unwrap(), path);
            return;
        }
        let err = result.unwrap_err();
        let message = err.to_string();
        let SocketPathError::InUse {
            path: in_use,
            user_defined,
        } = err;
        assert_eq!(in_use, path);
        assert_eq!(user_defined, configured);
        assert!(message.contains(&path.display().to_string()), "{message}");
        let hint = if configured {
            "has a unique value"
        } else {
            "set an explicit"
        };
        assert!(message.contains(hint), "{message}");
    }

    /// With `ATUIN_HOME` set, a socket left behind with nothing listening, as by a daemon that was
    /// killed, is free: the client uses it, and autostart replaces it.
    #[cfg(unix)]
    #[rstest]
    fn with_atuin_home_a_stale_socket_is_free(#[values(false, true)] configured: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        drop(listen(&path));
        assert!(path.exists(), "the stale socket should be left behind");
        assert_eq!(socket_path(&settings.daemon).unwrap(), path);
    }

    /// With `ATUIN_HOME` set, the socket the pidfile names is used while its daemon is running,
    /// whatever else exists.
    #[cfg(unix)]
    #[rstest]
    fn with_atuin_home_the_recorded_socket_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        let _guard = acquire(&daemon.daemon).unwrap();

        let client = settings(&pidfile, &dir.path().join("other.sock"), false);
        let _listener = listen(&dir.path().join("other.sock"));
        assert_eq!(socket_path(&client.daemon).unwrap(), dir.path().join("daemon.sock"));
    }

    /// With `ATUIN_HOME` set, a socket recorded by a daemon that has since exited isn't trusted:
    /// another profile's daemon may be using that path now. The client goes by its own settings,
    /// and refuses the path if something is live there.
    #[cfg(unix)]
    #[rstest]
    fn with_atuin_home_a_stale_pidfile_is_not_trusted(#[values(false, true)] live: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        // A daemon recorded `daemon.sock`, then exited, leaving its pidfile behind. Its path is
        // now live, as if another profile's daemon had taken it.
        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        drop(acquire(&daemon.daemon).unwrap());
        let _taken = listen(&dir.path().join("daemon.sock"));

        let client = settings_in(dir.path(), true);
        let path = preferred_path(dir.path(), true);
        let _listener = live.then(|| listen(&path));
        match socket_path(&client.daemon) {
            Ok(found) => {
                assert!(!live, "a live socket at {} was accepted", path.display());
                assert_eq!(found, path);
            }
            Err(SocketPathError::InUse { path: in_use, .. }) => {
                assert!(live, "a free socket path was refused");
                assert_eq!(in_use, path);
            }
        }
    }
}
