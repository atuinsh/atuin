//! Module which manages the atuin-daemon service.
//!
//! This is called `agent`, in-line with launchd's semantics of what an agent is, a daemon that runs
//! as part of a user account's process tree, as opposed to as part of root.
//!
//! Whichever backend runs the agent, the agent process is `atuin daemon start`, which holds the
//! pidfile locked for as long as it runs. Clients that replace or start the agent first take the
//! startup lock beside it, so that concurrent clients (a burst of shell hooks) start one agent
//! between them rather than one each.

mod backend;

use std::fs::File;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use atuin_client::settings::{Daemon as DaemonSettings, Settings};
use atuin_common::fs::lock::{LockError, LockMode, LockOptions};
pub use backend::{FindError, SpawnError, StopError};

use crate::agent::backend::{AgentHandle, AnyBackend, Backend};
#[cfg(unix)]
use crate::client::DaemonAddress;
use crate::grpc::versioning::OtherBuild;

/// Represents an active agent.
pub struct ActiveAgent {
    backend: AnyBackend,
    handle: AgentHandle,
    startup: StartupLock,
}

/// Errors returned by [`ActiveAgent::find`], [`ActiveAgent::spawn`] and
/// [`ActiveAgent::find_or_spawn`].
#[derive(Debug, thiserror::Error)]
pub enum AgentFindError {
    #[error(transparent)]
    StartupLock(#[from] StartupLockError),
    #[error(transparent)]
    Find(#[from] FindError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// Stopping an outdated agent to replace it failed.
    #[error(transparent)]
    Replace(#[from] StopError),
    #[error(transparent)]
    Mismatch(#[from] OtherBuild),
}

/// Errors returned by [`ActiveAgent::restart`].
#[derive(Debug, thiserror::Error)]
pub enum RestartError {
    #[error(transparent)]
    StartupLock(#[from] StartupLockError),
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Mismatch(#[from] OtherBuild),
}

impl ActiveAgent {
    /// Try to find the currently active agent.
    ///
    /// Creates a new agent if one is not already running, and replaces one older than this client.
    pub async fn find_or_spawn(settings: &Settings) -> Result<Self, AgentFindError> {
        let backend = AnyBackend::new(settings);
        let startup = StartupLock::new(settings);
        let _held = startup.acquire().await?;

        if let Some(found) = backend.find().await? {
            match found.ensure_our_build() {
                Ok(()) => {
                    return Ok(Self {
                        backend,
                        handle: found,
                        startup,
                    });
                }
                Err(OtherBuild::Outdated(_)) => backend.force_stop(found).await?,
                Err(newer @ OtherBuild::Newer(_)) => return Err(newer.into()),
            }
        }

        let handle = backend.spawn().await?;
        handle.ensure_our_build()?;
        Ok(Self {
            backend,
            handle,
            startup,
        })
    }

    /// Try to find the active agent, if one exists, whatever its version.
    pub async fn find(settings: &Settings) -> Result<Option<Self>, AgentFindError> {
        let backend = AnyBackend::new(settings);
        let Some(handle) = backend.find().await? else {
            return Ok(None);
        };

        Ok(Some(Self {
            backend,
            handle,
            startup: StartupLock::new(settings),
        }))
    }

    /// Create a new agent, assuming one does not already exist.
    pub async fn spawn(settings: &Settings) -> Result<Self, AgentFindError> {
        let backend = AnyBackend::new(settings);
        let startup = StartupLock::new(settings);
        let _held = startup.acquire().await?;

        let handle = backend.spawn().await?;
        handle.ensure_our_build()?;
        Ok(Self {
            backend,
            handle,
            startup,
        })
    }

    /// Get the socket path of the agent.
    #[cfg(unix)]
    #[must_use]
    pub fn socket(&self) -> &Path {
        let DaemonAddress::Socket(socket) = self.handle.address();
        socket
    }

    /// Restart the agent.
    pub async fn restart(self) -> Result<Self, RestartError> {
        let _held = self.startup.acquire().await?;

        // Another client may have replaced the agent since this one was found, in which case this
        // stops its replacement instead. That costs the replacement a restart, not correctness.
        self.backend.force_stop(self.handle).await?;

        let handle = self.backend.spawn().await?;
        handle.ensure_our_build()?;
        Ok(Self { handle, ..self })
    }

    /// Stop the agent, returning once it has exited.
    pub async fn stop(self) -> Result<(), StopError> {
        self.backend.force_stop(self.handle).await
    }
}

/// The directory the agent runs in.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentWorkingDir {
    /// `/`, so that the agent holds no directory open that would stop its filesystem from being
    /// unmounted, or that may since have been deleted.
    Root,
    /// The working directory of the client that starts the agent, because a path the agent uses
    /// is configured relative to it, and both must resolve it alike.
    Client,
}

#[cfg(unix)]
impl AgentWorkingDir {
    pub fn for_settings(settings: &Settings) -> Self {
        // The data dir holds the stores the agent finds through it: captured output, the AI
        // session sidecar and the Octavo queue.
        let data_dir = &settings.data_dir;
        // The config is read again once the agent has started, so its location counts too.
        let config_file = atuin_common::dirs::config_path("config.toml");
        let relative = [
            settings.db_path.as_path(),
            settings.record_store_path.as_path(),
            settings.key_path.as_path(),
            Path::new(&settings.daemon.pidfile_path),
            Path::new(&settings.logs.dir),
            Path::new(&settings.meta.db_path),
            data_dir.as_path(),
        ]
        .into_iter()
        .chain(settings.daemon.socket_path.as_deref())
        .chain([config_file.as_path()])
        .any(Path::is_relative);

        if relative {
            Self::Client
        } else {
            Self::Root
        }
    }

    pub fn resolve(self) -> io::Result<PathBuf> {
        match self {
            Self::Root => Ok(PathBuf::from("/")),
            Self::Client => std::env::current_dir(),
        }
    }
}

/// How long a client waits on the agent: for the previous one to exit, for a new one to answer, or
/// for another client doing either.
///
/// That is the configured `local_timeout`, but at least `500ms`, plus `2s` for the agent to boot.
#[derive(Clone, Copy, Debug)]
struct LifecycleTimeout(Duration);

impl LifecycleTimeout {
    const MIN_LOCAL_TIMEOUT: Duration = Duration::from_millis(500);
    const BOOT: Duration = Duration::from_secs(2);

    const fn get(self) -> Duration {
        self.0
    }
}

impl From<&Settings> for LifecycleTimeout {
    fn from(settings: &Settings) -> Self {
        Self(settings.local_timeout.max(Self::MIN_LOCAL_TIMEOUT).saturating_add(Self::BOOT))
    }
}

/// The pidfile the agent holds locked for as long as it runs.
#[derive(Debug)]
struct Pidfile {
    path: PathBuf,
}

/// Errors from waiting for the agent to release its pidfile.
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
    fn new(daemon: &DaemonSettings) -> Self {
        Self {
            path: PathBuf::from(&daemon.pidfile_path),
        }
    }

    fn startup_lock_path(&self) -> PathBuf {
        let mut os = self.path.as_os_str().to_os_string();
        os.push(".startup.lock");
        PathBuf::from(os)
    }

    /// Wait until no agent holds the pidfile.
    async fn released(&self, timeout: LifecycleTimeout) -> Result<(), PidfileWaitError> {
        let exclusive = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        };

        // Taking the lock is only how to learn that it is free; it is released again straight away.
        match exclusive.wait(&self.path, timeout.get()).await {
            Ok(_released) => Ok(()),
            Err(LockError::Timeout) => Err(PidfileWaitError::StillHeld {
                path: self.path.clone(),
                timeout: timeout.get(),
            }),
            Err(lock) => Err(PidfileWaitError::Lock {
                path: self.path.clone(),
                lock,
            }),
        }
    }
}

/// The lock a client holds while it replaces or starts the agent.
#[derive(Debug)]
struct StartupLock {
    path: PathBuf,
    timeout: LifecycleTimeout,
}

#[derive(Debug, thiserror::Error)]
#[error("failed to acquire daemon startup lock at {}: {lock}", .path.display())]
pub struct StartupLockError {
    path: PathBuf,
    lock: LockError,
}

impl StartupLock {
    fn new(settings: &Settings) -> Self {
        Self {
            path: Pidfile::new(&settings.daemon).startup_lock_path(),
            timeout: LifecycleTimeout::from(settings),
        }
    }

    /// Wait for the lock, which is held until the returned file is dropped.
    async fn acquire(&self) -> Result<File, StartupLockError> {
        LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .wait(&self.path, self.timeout.get())
        .await
        .map_err(|lock| StartupLockError {
            path: self.path.clone(),
            lock,
        })
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    const SHORT_TIMEOUT: LifecycleTimeout = LifecycleTimeout(Duration::from_millis(100));

    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn pidfile(dir: &TempDir) -> Pidfile {
        Pidfile {
            path: dir.path().join("atuin-daemon.pid"),
        }
    }

    #[rstest]
    #[case("/tmp/atuin-daemon.pid", "/tmp/atuin-daemon.pid.startup.lock")]
    #[case("/path/to/pidfile", "/path/to/pidfile.startup.lock")]
    fn startup_lock_sits_beside_the_pidfile(#[case] pidfile: &str, #[case] expected: &str) {
        let pidfile = Pidfile {
            path: PathBuf::from(pidfile),
        };
        assert_eq!(pidfile.startup_lock_path(), PathBuf::from(expected));
    }

    #[cfg(unix)]
    #[rstest]
    fn default_paths_keep_the_agent_at_the_root() {
        assert_eq!(AgentWorkingDir::for_settings(&Settings::default()), AgentWorkingDir::Root);
    }

    #[cfg(unix)]
    #[rstest]
    #[case::db(|s: &mut Settings| s.db_path = "history.db".into())]
    #[case::socket(|s: &mut Settings| s.daemon.socket_path = Some("atuin.sock".into()))]
    #[case::pidfile(|s: &mut Settings| s.daemon.pidfile_path = "atuin.pid".into())]
    #[case::meta(|s: &mut Settings| s.meta.db_path = "meta.db".into())]
    #[case::data_dir(|s: &mut Settings| s.data_dir = "data".into())]
    fn relative_paths_keep_the_client_directory(#[case] set: fn(&mut Settings)) {
        let mut settings = Settings::default();
        set(&mut settings);
        assert_eq!(AgentWorkingDir::for_settings(&settings), AgentWorkingDir::Client);
    }

    /// The config is read again once the agent has started, from `ATUIN_CONFIG_DIR` if it's set.
    #[cfg(unix)]
    #[rstest]
    #[case::absolute("/etc/atuin", AgentWorkingDir::Root)]
    #[case::relative("atuin-config", AgentWorkingDir::Client)]
    fn a_relative_config_dir_keeps_the_client_directory(
        #[case] config_dir: &str,
        #[case] expected: AgentWorkingDir,
    ) {
        let settings = Settings::default();
        let env = atuin_common::env::MockEnv::install();
        env.set("ATUIN_CONFIG_DIR", config_dir);
        assert_eq!(AgentWorkingDir::for_settings(&settings), expected);
    }

    /// Paths only the client uses don't tie the agent to its working directory.
    #[cfg(unix)]
    #[rstest]
    #[case::kv(|s: &mut Settings| s.kv.db_path = "kv.db".into())]
    #[case::scripts(|s: &mut Settings| s.scripts.db_path = "scripts.db".into())]
    #[case::ai(|s: &mut Settings| s.ai.db_path = "ai.db".into())]
    fn client_only_relative_paths_are_ignored(#[case] set: fn(&mut Settings)) {
        let mut settings = Settings::default();
        set(&mut settings);
        assert_eq!(AgentWorkingDir::for_settings(&settings), AgentWorkingDir::Root);
    }

    #[rstest]
    fn lifecycle_timeout_saturates_rather_than_panicking() {
        let settings = Settings {
            local_timeout: Duration::MAX,
            ..Settings::default()
        };
        assert_eq!(LifecycleTimeout::from(&settings).get(), Duration::MAX);
    }

    #[rstest]
    #[tokio::test]
    async fn a_pidfile_no_agent_holds_is_released(dir: TempDir) {
        pidfile(&dir).released(SHORT_TIMEOUT).await.unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn a_pidfile_an_agent_holds_is_not_released(dir: TempDir) {
        let pidfile = pidfile(&dir);
        let _agent = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .open(&pidfile.path)
        .unwrap();

        let err = pidfile.released(SHORT_TIMEOUT).await.unwrap_err();
        assert!(matches!(err, PidfileWaitError::StillHeld { .. }), "{err}");
    }
}
