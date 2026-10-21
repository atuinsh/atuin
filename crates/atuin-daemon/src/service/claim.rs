use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonListener;
use crate::service::backend::{AnyBackend, Backend, BackendError};
use crate::service::pidfile::{AcquireGuardError, Pidfile, PidfileGuard, PidfileInfo};
#[cfg(unix)]
use crate::service::socket_file::SocketFile;

/// What a daemon holds while it is the service: the pidfile, and the socket its backend handed it.
pub struct ServiceClaim {
    // Fields drop in order, and the socket has to go first: a daemon launched once the pidfile is
    // free binds the same path, and would lose its socket to this one.
    #[cfg(unix)]
    _socket: Option<SocketFile>,
    _pidfile: PidfileGuard,
}

/// Errors returned by [`ServiceClaim::take`].
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error(transparent)]
    Pidfile(#[from] AcquireGuardError),
    #[error(transparent)]
    Listen(#[from] BackendError),
}

impl ServiceClaim {
    /// Make this process the service `daemon` describes, returning the listener to serve it on.
    ///
    /// Call it from within the Tokio runtime, which keeps a socket it binds fresh until the claim
    /// is dropped.
    pub fn take(daemon: &DaemonSettings) -> Result<(Self, DaemonListener), ClaimError> {
        // Only the pidfile's holder may clear the sockets a dead daemon left behind.
        let pidfile = Pidfile::new(&daemon.pidfile_path).acquire(&PidfileInfo::current(daemon))?;
        let listening = AnyBackend::new(daemon).listen()?;

        let claim = Self {
            #[cfg(unix)]
            _socket: listening.socket,
            _pidfile: pidfile,
        };
        Ok((claim, listening.listener))
    }
}
