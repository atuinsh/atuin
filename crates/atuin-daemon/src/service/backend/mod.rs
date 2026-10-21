//! The backend used to manage the daemon service.
#[cfg(unix)]
mod external_systemd;
mod subproc;

use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use enum_dispatch::enum_dispatch;
#[cfg(unix)]
pub use external_systemd::ExternalSystemd;
#[cfg(unix)]
pub use subproc::DaemonizeError;
pub use subproc::Subproc;

use crate::service::PidfileWaitError;
use crate::service::address::{DaemonAddress, DaemonListener};
use crate::service::connection::{Connection, FindError};
#[cfg(unix)]
use crate::service::socket_file::SocketFile;

/// An error from a [`Backend`], each of which fails in its own ways.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct BackendError(Box<dyn std::error::Error + Send + Sync>);

impl BackendError {
    fn new(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(err))
    }
}

/// Errors from spawning a service.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("timed out waiting for daemon startup after {}ms", .0.as_millis())]
    NotReady(Duration),
    #[error(transparent)]
    Find(#[from] FindError),
}

/// Errors from stopping a service.
#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// The socket a backend hands the service it runs.
pub struct Listening {
    pub listener: DaemonListener,
    /// The socket file the service bound itself, which it owns until it exits.
    #[cfg(unix)]
    pub socket: Option<SocketFile>,
}

/// How a service is started and stopped, and where it listens.
///
/// Waiting for one to exit or answer is the same whatever runs it, and is left to the caller.
#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    /// Where the services this backend launches listen.
    fn address(&self) -> DaemonAddress;

    /// Where the running service listens, whichever started it.
    fn locate(&self) -> Result<DaemonAddress, BackendError>;

    /// Take up the socket to serve on, from within a service this backend runs.
    fn listen(&self) -> Result<Listening, BackendError>;

    /// Start a service, without waiting for it to answer.
    async fn launch(&self) -> Result<(), BackendError>;

    /// Ask `service` to stop, without waiting for it to exit.
    async fn stop(&self, service: &Connection) -> Result<(), BackendError>;

    /// Kill the running service, whether or not it answers, and remove what it left behind.
    async fn evict(&self) -> Result<(), BackendError>;
}

#[enum_dispatch(Backend)]
pub enum AnyBackend {
    #[cfg(unix)]
    ExternalSystemd(ExternalSystemd),
    Subproc(Subproc),
}

impl AnyBackend {
    /// The backend that manages the service `daemon` describes.
    pub fn new(daemon: &DaemonSettings) -> Self {
        #[cfg(unix)]
        if daemon.systemd_socket {
            return ExternalSystemd::new(daemon).into();
        }

        Subproc::new(daemon).into()
    }
}
