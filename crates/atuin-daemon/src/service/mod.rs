//! Module which manages the running atuin-daemon service.
//!
//! You can have multiple different backends which manage the service.

pub mod address;
mod backend;
mod claim;
mod connection;
pub mod pidfile;
#[cfg(unix)]
mod socket_file;
mod startup_lock;

use std::time::Duration;

use atuin_client::settings::{Daemon as DaemonSettings, Settings};
#[cfg(unix)]
pub use backend::DaemonizeError;
pub use backend::{SpawnError, StopError, Subproc};
pub use claim::{ClaimError, ServiceClaim};
pub use connection::FindError;
use pidfile::Pidfile;
pub use pidfile::PidfileWaitError;
#[cfg(unix)]
pub use socket_file::RemoveSocketError;
use startup_lock::StartupLock;
pub use startup_lock::StartupLockError;

use crate::HistoryClient;
use crate::grpc::versioning::VersionMatchError;
use crate::service::address::DaemonAddress;
use crate::service::backend::{AnyBackend, Backend};
use crate::service::connection::Connection;

/// Represents an active service.
pub struct ActiveService {
    backend: AnyBackend,
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
    /// Stopping an outdated service to replace it failed.
    #[error(transparent)]
    Replace(#[from] StopError),
    #[error("daemon is newer than this client; leaving it running: {0}")]
    Newer(VersionMatchError),
    #[error(transparent)]
    Mismatch(#[from] VersionMatchError),
}

/// Errors returned by [`ActiveService::restart`].
#[derive(Debug, thiserror::Error)]
pub enum RestartError {
    #[error(transparent)]
    StartupLock(#[from] StartupLockError),
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Mismatch(#[from] VersionMatchError),
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
        let unready = match Self::find(settings).await? {
            None => ReadyError::NotRunning,
            Some(service) => match service.ensure_our_build() {
                Ok(()) => return Ok(service),
                Err(newer) if newer.peer_is_newer() => {
                    return Err(ServiceFindError::Newer(newer).into());
                }
                Err(outdated) => ReadyError::NeedsRestart(outdated),
            },
        };

        // Without `autostart`, something else manages the service.
        if !settings.daemon.autostart {
            return Err(unready);
        }

        Ok(Self::find_or_spawn(settings).await?)
    }

    /// Try to find the currently active service.
    ///
    /// Creates a new service if one is not already running, and replaces one older than this client.
    ///
    /// TODO(markovejnovic): Do not use the global Settings but pass DaemonSettings.
    pub async fn find_or_spawn(settings: &Settings) -> Result<Self, ServiceFindError> {
        let backend = AnyBackend::new(&settings.daemon);
        let pidfile = Pidfile::new(&settings.daemon.pidfile_path);
        let timeout = Self::timeout(settings.local_timeout);
        let startup = pidfile.startup_lock(timeout);
        let _held = startup.acquire().await?;

        if let Some(found) = Self::find(settings).await? {
            match found.ensure_our_build() {
                Ok(()) => return Ok(found),
                Err(newer) if newer.peer_is_newer() => return Err(ServiceFindError::Newer(newer)),
                Err(_outdated) => found.halt().await?,
            }
        }

        let connection = Self::launch(&backend, &pidfile, timeout).await?;
        connection.ensure_our_build()?;
        Ok(Self {
            backend,
            connection,
            pidfile,
            startup,
            timeout,
        })
    }

    /// Try to find the active service, if one exists, whatever its version.
    ///
    /// TODO(markovejnovic): Do not use the global Settings but pass DaemonSettings.
    pub async fn find(settings: &Settings) -> Result<Option<Self>, ServiceFindError> {
        let backend = AnyBackend::new(&settings.daemon);
        let Some(connection) = Connection::at(backend.locate().map_err(FindError::from)?).await?
        else {
            return Ok(None);
        };

        let pidfile = Pidfile::new(&settings.daemon.pidfile_path);
        let timeout = Self::timeout(settings.local_timeout);
        let startup = pidfile.startup_lock(timeout);
        Ok(Some(Self {
            backend,
            connection,
            pidfile,
            startup,
            timeout,
        }))
    }

    /// Kill the service `daemon` describes, whether or not it answers, and remove what it left
    /// behind, as far as its backend allows.
    ///
    /// Best effort: what it can't kill or remove, it logs.
    pub async fn evict(daemon: &DaemonSettings) {
        if let Err(e) = AnyBackend::new(daemon).evict().await {
            tracing::warn!("{e}");
        }
    }

    /// Where the running service `daemon` describes listens, whichever started it.
    pub fn locate(daemon: &DaemonSettings) -> Result<DaemonAddress, FindError> {
        Ok(AnyBackend::new(daemon).locate()?)
    }

    /// Check that the service is this client's build.
    pub fn ensure_our_build(&self) -> Result<(), VersionMatchError> {
        self.connection.ensure_our_build()
    }

    /// The connection to the service.
    #[must_use]
    pub fn into_client(self) -> HistoryClient {
        self.connection.history_client()
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
        self.halt().await?;

        let connection = Self::launch(&self.backend, &self.pidfile, self.timeout).await?;
        connection.ensure_our_build()?;
        Ok(Self { connection, ..self })
    }

    /// Stop the service, returning once it has exited.
    pub async fn stop(self) -> Result<(), StopError> {
        self.halt().await
    }

    /// Equivalent to [`Self::stop`], but keeps `self` to launch the next service with.
    async fn halt(&self) -> Result<(), StopError> {
        self.backend.stop(&self.connection).await?;
        self.pidfile.released(self.timeout).await?;
        Ok(())
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
        // launched before then would find the pidfile taken and exit itself.
        pidfile.released(timeout).await?;
        backend.launch().await?;
        Connection::connect_when_ready(&backend.address(), timeout).await
    }
}
