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
use std::path::{Path, PathBuf};

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::fs::lock::{LockError, LockMode, LockOptions};
use atuin_common::serde::AutoOsString;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

const FORMAT_V1: &str = "v1";

/// The path to the lock used to guard reads and writes of the pidfile itself.
#[must_use]
fn lock_path(pidfile_path: &Path) -> PathBuf {
    let mut os = pidfile_path.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

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
    /// This is [`None`] on Windows.
    #[serde_as(as = "Option<AutoOsString>")]
    pub socket_path: Option<PathBuf>,
}

impl PidfileInfo {
    /// Create a [`PidfileInfo`] for the current process (assuming this process is the daemon).
    #[must_use]
    pub fn current<P>(socket_path: Option<P>) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            socket_path: socket_path.map(Into::into),
        }
    }

    /// Read the [`PidfileInfo`] in the given pidfile.
    #[must_use]
    pub fn read(pidfile_path: &Path) -> Option<Self> {
        let options = LockOptions {
            // If the pidfile lock doesn't exist, we return early (no pidfile lock means there's no
            // daemon running or the daemon is an older version that doesn't use the v1 format), so
            // there's no point in creating the pidfile lock.
            create: false,
            mode: LockMode::Shared,
        };

        let lock_path = lock_path(pidfile_path);
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

        let contents = match std::fs::read_to_string(pidfile_path) {
            Ok(contents) => contents,
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::debug!(
                        "could not read daemon pidfile at {}: {e}",
                        pidfile_path.display()
                    );
                }
                return None;
            }
        };
        drop(lock);

        Self::parse(&contents)
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

    /// Replace the contents of the pidfile with this [`PidfileInfo`].
    fn write(&self, pidfile: &mut File, pidfile_path: &Path) -> Result<(), WritePidfileError> {
        let json = serde_json::to_string(self)?;
        let lock_path = lock_path(pidfile_path);

        let _lock = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .open(&lock_path)
        .map_err(|e| WritePidfileError::Lock {
            path: lock_path,
            inner: e,
        })?;

        pidfile
            .set_len(0)
            .and_then(|()| {
                // `set_len` doesn't move the cursor; we need to do that separately.
                pidfile.rewind()?;
                writeln!(pidfile, "{}\n{FORMAT_V1}\n{json}", self.pid)?;
                pidfile.flush()
            })
            .map_err(|e| WritePidfileError::Write {
                path: pidfile_path.into(),
                inner: e,
            })
    }
}

/// An error that occurred while trying to [acquire] a [`PidfileGuard`].
///
/// [acquire]: PidfileGuard::acquire
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

impl PidfileGuard {
    /// Lock the daemon pidfile and write this daemon's information into it.
    ///
    /// TODO(taylordotfish): On Windows, this locks the entire byte range of the pidfile, which
    /// means that while the lock is held, no other process can read the contents of the pidfile.
    /// This means that `force_cleanup` in `atuin/src/command/client/daemon.rs` can never
    /// successfully kill a running daemon, because either:
    ///
    /// * The daemon is running, which means the lock is held, which means the client can't read the
    ///   daemon's PID from the pidfile; or
    /// * The daemon isn't running, so the client can read the pidfile, but the PID in the pidfile
    ///   necessarily isn't running.
    ///
    /// This is not a critical issue -- it only affects the `force_cleanup` path in which somethis
    /// has already gone wrong -- but it would still be good to fix. A potential approach would be
    /// to use `windows-sys` to lock a single byte well past the actual contents of the file. SQLite
    /// uses this approach; its locks start at 0x40000000.
    pub fn acquire(settings: &DaemonSettings) -> Result<Self, AcquireGuardError> {
        let path = Path::new(&settings.pidfile_path);

        let mut file = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .try_open(path)
        .map_err(|e| LockPidfileError {
            path: path.into(),
            inner: e,
        })?;

        #[cfg(unix)]
        let socket_path = Some(settings.socket_path().into_owned());
        #[cfg(not(unix))]
        let socket_path = None::<PathBuf>;

        PidfileInfo::current(socket_path).write(&mut file, path)?;

        Ok(Self { _file: file })
    }
}

/// Try to read the daemon PID from the given pidfile.
///
/// This handles both the new v1 format and the old format.
#[must_use]
pub fn try_read_pid(pidfile_path: &Path) -> Option<u32> {
    let lock_path = lock_path(pidfile_path);

    let _lock = LockOptions {
        create: true,
        mode: LockMode::Shared,
    }
    .open(&lock_path)
    .inspect_err(|e| tracing::debug!("could not lock pidfile lock at {}: {e}", lock_path.display()))
    .ok()?;

    let contents = std::fs::read_to_string(pidfile_path).ok()?;
    let pid = contents.lines().next()?.trim();
    pid.parse()
        .inspect_err(|e| {
            tracing::debug!(
                "failed to parse {pid:?} as pid in pidfile at {}: {e}",
                pidfile_path.display()
            );
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

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

    fn info() -> PidfileInfo {
        PidfileInfo::current(Some("/tmp/atuin-1000/atuin.sock"))
    }

    fn daemon_settings(pidfile: &Path) -> DaemonSettings {
        DaemonSettings {
            pidfile_path: pidfile.to_str().unwrap().to_string(),
            socket_path: Some("/tmp/atuin.sock".into()),
            ..DaemonSettings::default()
        }
    }

    #[rstest]
    fn test_guard_acquire_and_drop(pidfile: Pidfile) {
        {
            let _guard = PidfileGuard::acquire(&daemon_settings(&pidfile.path)).unwrap();
            // Guard holds an exclusive lock — on Windows other handles cannot
            // read the file, so we verify contents after the guard is dropped.
        }

        let contents = std::fs::read_to_string(&pidfile.path).unwrap();
        assert_eq!(contents.lines().next().unwrap(), std::process::id().to_string());
        let info = PidfileInfo::read(&pidfile.path).unwrap();
        assert_eq!(info.version, crate::VERSION);
        assert_eq!(info.socket_path.as_deref(), Some("/tmp/atuin.sock".as_ref()));

        // After guard is dropped, lock should be released — acquiring again must succeed.
        let _guard2 = PidfileGuard::acquire(&daemon_settings(&pidfile.path)).unwrap();
    }

    #[rstest]
    fn test_guard_prevents_double_acquire(pidfile: Pidfile) {
        let settings = daemon_settings(&pidfile.path);
        let _guard = PidfileGuard::acquire(&settings).unwrap();
        let result = PidfileGuard::acquire(&settings);
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
            lock_path(Path::new("/tmp/atuin-daemon.pid")),
            PathBuf::from("/tmp/atuin-daemon.pid.lock")
        );
    }

    #[rstest]
    fn test_write_format(pidfile: Pidfile) {
        info().write(&mut pidfile.create(), &pidfile.path).unwrap();

        let contents = std::fs::read_to_string(&pidfile.path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "got: {contents:?}");
        assert_eq!(lines[0], std::process::id().to_string());
        assert_eq!(lines[1], FORMAT_V1);
        assert!(lines[2].contains(r#""/tmp/atuin-1000/atuin.sock""#), "got: {contents:?}");
        assert!(lock_path(&pidfile.path).exists());
    }

    #[rstest]
    fn test_write_replaces_longer_contents(pidfile: Pidfile) {
        let mut file = pidfile.create();
        let long = PidfileInfo::current(Some("/a/much/longer/path/than/the/next/one.sock"));
        long.write(&mut file, &pidfile.path).unwrap();
        info().write(&mut file, &pidfile.path).unwrap();

        assert_eq!(PidfileInfo::read(&pidfile.path), Some(info()));
    }

    #[rstest]
    #[case::some(Some("/tmp/atuin-1000/atuin.sock".into()))]
    #[case::none(None)]
    fn test_round_trip(pidfile: Pidfile, #[case] socket_path: Option<PathBuf>) {
        let info = PidfileInfo::current(socket_path);
        info.write(&mut pidfile.create(), &pidfile.path).unwrap();
        assert_eq!(PidfileInfo::read(&pidfile.path), Some(info));
    }

    #[rstest]
    fn test_read_without_lock_file(pidfile: Pidfile) {
        info().write(&mut pidfile.create(), &pidfile.path).unwrap();
        std::fs::remove_file(lock_path(&pidfile.path)).unwrap();

        assert_eq!(PidfileInfo::read(&pidfile.path), None);
        assert!(!lock_path(&pidfile.path).exists(), "read must not create the lock");
    }

    #[rstest]
    fn test_read_without_pidfile(pidfile: Pidfile) {
        File::create(lock_path(&pidfile.path)).unwrap();
        assert_eq!(PidfileInfo::read(&pidfile.path), None);
    }

    #[rstest]
    #[case::legacy("1234\n18.9.0\n")]
    #[case::pid_only("1234\n")]
    #[case::half_written("1234\nv1\n{\"version\":\"18.9")]
    #[case::bad_json("1234\nv1\nnot json\n")]
    #[case::empty("")]
    fn test_read_unusable(pidfile: Pidfile, #[case] contents: &str) {
        File::create(lock_path(&pidfile.path)).unwrap();
        std::fs::write(&pidfile.path, contents).unwrap();
        assert_eq!(PidfileInfo::read(&pidfile.path), None);
    }

    #[rstest]
    fn test_read_ignores_unknown_fields(pidfile: Pidfile) {
        File::create(lock_path(&pidfile.path)).unwrap();
        std::fs::write(
            &pidfile.path,
            "1234\nv1\n{\"version\":\"1.2.3\",\"socket_path\":null,\"future\":true}\n",
        )
        .unwrap();
        assert_eq!(
            PidfileInfo::read(&pidfile.path),
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
}
