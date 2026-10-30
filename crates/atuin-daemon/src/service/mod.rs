//! Module which manages the running atuin-daemon service.
//!
//! You can have multiple different backends which manage the service.

pub mod address;
mod backend;
mod claim;
mod connection;
pub mod pidfile;
mod run_mode;
mod socket_holder;
mod startup_lock;
mod systemd;

use std::sync::OnceLock;
use std::time::Duration;

use atuin_client::settings::{Daemon as DaemonSettings, Settings};
use atuin_common::userio::log::RequestId;
pub use backend::{DaemonizeError, SpawnError, StopError, Subproc};
pub use claim::{ClaimError, ServiceClaim};
pub use connection::FindError;
use pidfile::Pidfile;
pub use pidfile::PidfileWaitError;
pub use run_mode::{ExternalManager, Launch, RunMode};
pub use socket_holder::ListenError;
use startup_lock::StartupLock;
pub use startup_lock::StartupLockError;

use crate::HistoryClient;
use crate::grpc::request;
use crate::grpc::versioning::VersionMatchError;
use crate::service::address::DaemonAddress;
use crate::service::backend::{AnyBackend, Backend, Launchd};
use crate::service::connection::Connection;

/// Represents an active service.
pub struct ActiveService {
    daemon: DaemonSettings,
    /// Worked out when first needed to launch or stop the service, which most clients never do:
    /// that asks the pidfile what runs the service, and whether the launchd agent is installed.
    backend: OnceLock<AnyBackend>,
    connection: Connection,
    pidfile: Pidfile,
    startup: StartupLock,
    timeout: Duration,
}

/// Errors returned by [`ActiveService::find`] and [`ActiveService::find_or_spawn`].
#[derive(Debug, thiserror::Error)]
pub enum ServiceFindError {
    #[error(transparent)]
    StartupLock(#[from] StartupLockError),
    #[error(transparent)]
    Find(#[from] FindError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// Replacing an outdated or unresponsive service failed.
    #[error(transparent)]
    Replace(#[from] ReplaceError),
    #[error("daemon is newer than this client; leaving it running: {0}")]
    Newer(VersionMatchError),
}

/// Errors returned by [`ActiveService::restart`].
#[derive(Debug, thiserror::Error)]
pub enum RestartError {
    #[error(transparent)]
    StartupLock(#[from] StartupLockError),
    #[error(transparent)]
    Replace(#[from] ReplaceError),
}

/// Errors from replacing a service with one of this client's build.
#[derive(Debug, thiserror::Error)]
pub enum ReplaceError {
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

/// Errors returned by [`ActiveService::ready`].
#[derive(Debug, thiserror::Error)]
pub enum ReadyError {
    #[error(transparent)]
    Find(#[from] ServiceFindError),
    #[error("no daemon is running; enable `daemon.autostart = true` or run `atuin daemon start`")]
    NotRunning,
    #[error(
        "daemon is out of date: {0}. Enable `daemon.autostart = true` or restart the daemon \
         manually"
    )]
    NeedsRestart(VersionMatchError),
}

impl ActiveService {
    /// The service of this client's build, starting it, or replacing one older than this client,
    /// when `daemon.autostart` allows.
    ///
    /// TODO(markovejnovic): Do not use the global Settings but pass DaemonSettings.
    pub async fn ready(settings: &Settings) -> Result<Self, ReadyError> {
        let unready = match Self::find(settings).await {
            Ok(None) => ReadyError::NotRunning,
            Ok(Some(service)) => match service.ensure_our_build() {
                Ok(()) => return Ok(service),
                Err(newer) if newer.peer_is_newer() => {
                    return Err(ServiceFindError::Newer(newer).into());
                }
                Err(outdated) => ReadyError::NeedsRestart(outdated),
            },
            // `find_or_spawn` replaces one that can't answer.
            Err(ServiceFindError::Find(unresponsive @ FindError::Unresponsive(_))) => {
                ReadyError::Find(unresponsive.into())
            }
            Err(err) => return Err(err.into()),
        };

        // Without `autostart`, something else manages the service.
        if !settings.daemon.autostart {
            return Err(unready);
        }

        Ok(Self::find_or_spawn(settings).await?)
    }

    /// Try to find the currently active service.
    ///
    /// Creates a new service if one is not already running, and replaces one older than this client,
    /// or one that can't answer while no service holds the pidfile.
    ///
    /// TODO(markovejnovic): Do not use the global Settings but pass DaemonSettings.
    pub async fn find_or_spawn(settings: &Settings) -> Result<Self, ServiceFindError> {
        let backend = AnyBackend::new(&settings.daemon);
        let pidfile = Pidfile::new(&settings.daemon.pidfile_path);
        let timeout = Self::timeout(settings.local_timeout);
        let startup = pidfile.startup_lock(timeout);
        let _held = startup.acquire().await?;

        match Self::find(settings).await {
            Ok(Some(found)) => match found.ensure_our_build() {
                Ok(()) => return Ok(found),
                Err(newer) if newer.peer_is_newer() => return Err(ServiceFindError::Newer(newer)),
                Err(_outdated) => {
                    let connection = found.replace().await?;
                    return Ok(Self {
                        connection,
                        ..found
                    });
                }
            },
            Ok(None) => {}
            // launchd holds the socket for an agent that can't start, such as one whose executable
            // was removed. A service holding the pidfile may only be slow to boot, and killing it
            // would leave the next client to kill the next one.
            Err(ServiceFindError::Find(FindError::Unresponsive(_))) if !pidfile.is_live() => {
                backend.evict().await.map_err(|err| ReplaceError::Stop(err.into()))?;
            }
            Err(err) => return Err(err),
        }

        let connection = Self::launch(&backend, &pidfile, timeout).await?;
        Ok(Self {
            daemon: settings.daemon.clone(),
            backend: OnceLock::from(backend),
            connection,
            pidfile,
            startup,
            timeout,
        })
    }

    /// Try to find the active service, if one exists, whatever its version.
    ///
    /// It starts none itself, but connecting to the launchd agent's socket has launchd start the
    /// agent if it isn't running.
    ///
    /// TODO(markovejnovic): Do not use the global Settings but pass DaemonSettings.
    pub async fn find(settings: &Settings) -> Result<Option<Self>, ServiceFindError> {
        /// How long a socket may go unanswered while no daemon holds the pidfile. A daemon takes it
        /// moments after it starts, before it answers, so after this nothing is about to. Short
        /// enough that `history start`, which gives up after 2s, still gets to replace it.
        const UNCLAIMED: Duration = Duration::from_millis(500);

        let timeout = Self::timeout(settings.local_timeout);
        let pidfile = Pidfile::new(&settings.daemon.pidfile_path);
        let address = AnyBackend::locate(&settings.daemon).map_err(FindError::from)?;
        // A socket can accept connections with nothing behind it to answer them, as launchd's does
        // while the agent it starts fails to boot.
        let mut answer = std::pin::pin!(Connection::at(address));
        let found = match tokio::time::timeout(UNCLAIMED, &mut answer).await {
            Ok(found) => found,
            // A daemon that takes the pidfile just after this is taken for none, but
            // `find_or_spawn` looks again before it replaces one.
            Err(_) if !pidfile.is_live() => return Err(FindError::Unresponsive(UNCLAIMED).into()),
            Err(_) => tokio::time::timeout(timeout.saturating_sub(UNCLAIMED), answer)
                .await
                .map_err(|_| FindError::Unresponsive(timeout))?,
        };
        let Some(connection) = found? else {
            return Ok(None);
        };

        let startup = pidfile.startup_lock(timeout);
        Ok(Some(Self {
            daemon: settings.daemon.clone(),
            backend: OnceLock::new(),
            connection,
            pidfile,
            startup,
            timeout,
        }))
    }

    /// Kill the service `daemon` describes, whether or not it answers, and remove what it left
    /// behind, as far as its backend allows.
    ///
    /// Fails only when its backend can't stop it at all: what it can't kill or remove, it logs.
    pub async fn evict(daemon: &DaemonSettings) -> Result<(), StopError> {
        // A daemon that takes the pidfile in the instant between choosing the backend and evicting
        // may be evicted however it runs.
        Ok(AnyBackend::new(daemon).evict().await?)
    }

    /// Remove the launchd agent, which launchd would otherwise start again at the next login, for a
    /// user turning the daemon off.
    pub async fn remove_launchd_agent() -> Result<(), StopError> {
        Ok(Launchd::uninstall().await?)
    }

    /// Where the running service `daemon` describes listens, whichever started it.
    pub fn locate(daemon: &DaemonSettings) -> Result<DaemonAddress, FindError> {
        Ok(AnyBackend::locate(daemon)?)
    }

    /// Check that the service is this client's build.
    pub fn ensure_our_build(&self) -> Result<(), VersionMatchError> {
        self.connection.ensure_our_build()
    }

    /// A client of the service's history API.
    #[must_use]
    pub fn history_client(&self) -> HistoryClient {
        self.connection.history_client()
    }

    /// Runs `request`, showing on this process's [`SINK`](atuin_common::userio::log::SINK) what the
    /// service logs for the user while serving the calls it makes. A service that can't send its
    /// logs still runs `request`.
    pub async fn with_logs<R>(&self, request: impl Future<Output = R>) -> R {
        /// How long the service has to start sending its logs: as long as it has to answer at all
        /// when it is looked for.
        const LISTEN_WAIT: Duration = Duration::from_secs(2);

        request::scope(RequestId::generate(), async {
            let logs = tokio::time::timeout(LISTEN_WAIT, self.connection.listen_logs()).await;
            let result = request.await;
            match logs {
                Ok(Ok(logs)) => logs.finish().await,
                Ok(Err(status)) => tracing::debug!(?status, "not listening to the daemon's logs"),
                Err(_) => tracing::debug!("the daemon didn't start sending its logs in time"),
            }
            result
        })
        .await
    }

    /// Where the service listens.
    #[must_use]
    pub const fn address(&self) -> &DaemonAddress {
        self.connection.address()
    }

    /// Restart the service.
    pub async fn restart(self) -> Result<Self, RestartError> {
        let _held = self.startup.acquire().await?;

        // TODO(markovejnovic): `self.connection` predates the lock. A client that replaced the
        //                      service meanwhile at another socket (e.g. another `TMPDIR`) leaves
        //                      this shutting down the dead old one and timing out on the pidfile
        //                      the replacement holds. Look the service up again under the lock.
        let connection = self.replace().await?;
        Ok(Self { connection, ..self })
    }

    /// Stop the service, returning once it has exited.
    pub async fn stop(self) -> Result<(), StopError> {
        self.halt().await
    }

    /// Replace the service with one of this client's build, and connect to that.
    async fn replace(&self) -> Result<Connection, ReplaceError> {
        let backend = self.backend();
        match backend {
            // Stopped, it would be started again behind the back of one launched here.
            AnyBackend::External(external) => {
                external.restart().await.map_err(SpawnError::from)?;
                // Where the manager runs it, which needn't be where this client would, the new one
                // says in the pidfile once it holds it.
                let locate = || Self::locate(&self.daemon);
                Ok(Connection::connect_when_ready(locate, self.timeout).await?)
            }
            // Installed again, the agent runs this client's executable.
            AnyBackend::Launchd(_) | AnyBackend::Subproc(_) => {
                self.halt().await?;
                Ok(Self::launch(backend, &self.pidfile, self.timeout).await?)
            }
        }
    }

    /// Equivalent to [`Self::stop`], but keeps `self` to launch the next service with.
    async fn halt(&self) -> Result<(), StopError> {
        self.backend().stop(&self.connection).await?;
        self.pidfile.released(self.timeout).await?;
        Ok(())
    }

    fn backend(&self) -> &AnyBackend {
        self.backend.get_or_init(|| AnyBackend::new(&self.daemon))
    }

    /// How long a client waits on the service: for the previous one to exit, for a new one to
    /// answer, or for another client doing either.
    ///
    /// That is the configured `local_timeout`, but at least `500ms`, plus `2s` for the service to
    /// boot.
    fn timeout(local_timeout: Duration) -> Duration {
        const MIN_LOCAL_TIMEOUT: Duration = Duration::from_millis(500);
        const BOOT: Duration = Duration::from_secs(2);

        local_timeout.max(MIN_LOCAL_TIMEOUT).saturating_add(BOOT)
    }

    /// Launch a service through `backend` once the last one has exited, and wait for it to answer.
    async fn launch(
        backend: &AnyBackend,
        pidfile: &Pidfile,
        timeout: Duration,
    ) -> Result<Connection, SpawnError> {
        // A service that was just asked to shut down holds the pidfile until it exits, and one
        // launched before then would find the pidfile taken and exit itself. Under launchd, the new
        // service's socket would also replace the one the last service still answers on.
        pidfile.released(timeout).await?;
        backend.launch().await?;
        Connection::connect_when_ready(|| Ok(backend.address()), timeout).await
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;

    use super::*;
    use crate::service::pidfile::PidfileInfo;

    /// A socket that accepts connections but never answers, as launchd's does while its agent fails
    /// to boot, is given up on early while no daemon holds the pidfile: one that does may only be
    /// slow.
    #[cfg(unix)]
    #[rstest]
    #[case::unclaimed(false)]
    #[case::claimed(true)]
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_socket_is_waited_on_only_while_a_daemon_holds_the_pidfile(
        #[case] claimed: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("atuin.sock");
        let mut settings = Settings::default();
        settings.daemon.pidfile_path =
            AbsPathBuf::new(dir.path().join("atuin-daemon.pid")).unwrap();
        settings.daemon.socket_path = Some(AbsPathBuf::new(socket.clone()).unwrap());
        // Listening, it takes connections into its backlog, but nothing accepts them.
        let _socket = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let info = PidfileInfo {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            socket_path: Some(socket),
            run_mode: Some(RunMode::DaemonizeForked),
            program: None,
        };
        let _daemon =
            claimed.then(|| Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap());

        let Err(ServiceFindError::Find(FindError::Unresponsive(waited))) =
            ActiveService::find(&settings).await
        else {
            panic!("a socket that never answers should be unresponsive");
        };
        assert_eq!(waited == ActiveService::timeout(settings.local_timeout), claimed);
    }
}
