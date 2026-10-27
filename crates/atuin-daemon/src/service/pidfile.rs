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
//! file, the *pidfile lock*, which is held whenever the pidfile is being taken, written to or read
//! from. It has the same path as the pidfile but with `.lock` appended. Unlike the pidfile itself,
//! which is locked for long periods of time by the daemon, the pidfile lock should be held for the
//! minimum amount of time necessary -- only while taking, reading from or writing to the pidfile.

use std::fmt::{self, Display};
use std::fs::File;
use std::io::{self, Seek, Write};
use std::path::PathBuf;
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::fs::lock::{LockError, LockMode, LockOptions};
use atuin_common::string::AutoOsString;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::service::address::DaemonAddress;
use crate::service::run_mode::RunMode;
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

    /// How the daemon runs, and so who starts and stops it. [`None`] for a daemon from before it
    /// was recorded.
    #[serde(default)]
    pub run_mode: Option<RunMode>,

    /// The path the daemon was run as, which what runs it runs again to restart it. [`None`] if
    /// that path was relative, or for a daemon from before it was recorded.
    #[serde_as(as = "Option<AutoOsString>")]
    pub program: Option<PathBuf>,
}

impl PidfileInfo {
    /// Create a [`PidfileInfo`] for the current process (assuming this process is the daemon),
    /// which runs as `run_mode`.
    #[must_use]
    pub fn current(settings: &DaemonSettings, run_mode: RunMode) -> Self {
        Self {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            // systemd's socket is wherever its unit puts it.
            socket_path: if settings.systemd_socket {
                None
            } else {
                DaemonAddress::configured(settings).into_socket_path()
            },
            run_mode: Some(run_mode),
            program: std::env::args_os()
                .next()
                .map(PathBuf::from)
                .filter(|program| program.is_absolute()),
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
        // Taken before the pidfile, so that no client reads a pidfile that is held but still names
        // the daemon before, and signals that one's PID or leaves this one to the wrong manager.
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

    /// Replace the contents of the pidfile, open as `file`, with `info`, under the pidfile lock.
    fn write(&self, file: &mut File, info: &PidfileInfo) -> Result<(), WritePidfileError> {
        let json = serde_json::to_string(info)?;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atuin_common::path::AbsPathBuf;
    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::*;

    struct Pidfile {
        _dir: tempfile::TempDir,
        path: PathBuf,
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
            run_mode: Some(RunMode::DaemonizeForked),
            program: Some(PathBuf::from("/opt/homebrew/opt/atuin/bin/atuin")),
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
        drop(at(&pidfile).acquire(&info()).unwrap());

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
        let long = info_with(Some("/a/much/longer/path/than/the/next/one.sock"));
        drop(at(&pidfile).acquire(&long).unwrap());
        drop(at(&pidfile).acquire(&info()).unwrap());

        assert_eq!(at(&pidfile).info(), Some(info()));
    }

    #[rstest]
    #[case::some(Some("/tmp/atuin-1000/atuin.sock"))]
    #[case::none(None)]
    fn test_round_trip(pidfile: Pidfile, #[case] socket_path: Option<&str>) {
        let info = info_with(socket_path);
        drop(at(&pidfile).acquire(&info).unwrap());
        assert_eq!(at(&pidfile).info(), Some(info));
    }

    #[rstest]
    fn test_read_without_lock_file(pidfile: Pidfile) {
        drop(at(&pidfile).acquire(&info()).unwrap());
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
                socket_path: None,
                run_mode: None,
                program: None,
            })
        );
    }

    proptest! {
        #[rstest]
        fn prop_parse_round_trip(
            pid in any::<u32>(),
            version in ".*",
            // `AutoOsString`'s own tests round-trip the paths that aren't UTF-8.
            socket_path in proptest::option::of(".*".prop_map(PathBuf::from)),
            run_mode in proptest::option::of(prop_oneof![
                Just(RunMode::DaemonizeForked),
                Just(RunMode::LaunchdAgent),
                proptest::option::of(prop_oneof![
                    Just(crate::service::ExternalManager::SystemdSocket),
                    ".*".prop_map(|name| crate::service::ExternalManager::SystemdUnit { name }),
                    ".*".prop_map(|label| crate::service::ExternalManager::LaunchdJob { label }),
                ])
                .prop_map(RunMode::External),
            ]),
            program in proptest::option::of(".*".prop_map(PathBuf::from)),
        ) {
            let info = PidfileInfo { pid, version, socket_path, run_mode, program };
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
        super::Pidfile::new(&daemon.pidfile_path)
            .acquire(&PidfileInfo::current(daemon, RunMode::DaemonizeForked))
    }
}
