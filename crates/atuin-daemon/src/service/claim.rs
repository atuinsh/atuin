use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonListener;
use crate::service::backend::{BackendError, Launchd};
use crate::service::pidfile::{AcquireGuardError, Pidfile, PidfileGuard, PidfileInfo};
use crate::service::run_mode::{ExternalManager, Launch, RunMode};
#[cfg(unix)]
use crate::service::socket_file::SocketFile;
use crate::service::socket_holder::{ListenError, SocketHolder};

/// What a daemon holds while it is the service: the pidfile, and the socket it serves on.
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
    CheckIn(#[from] BackendError),
    #[error(transparent)]
    Pidfile(#[from] AcquireGuardError),
    #[error(transparent)]
    Listen(#[from] ListenError),
}

impl ServiceClaim {
    /// Make this process, `launch`ed so, the service `daemon` describes, returning the listener to
    /// serve it on.
    ///
    /// The runtime it runs on keeps a socket it binds fresh until the claim is dropped.
    pub async fn take(
        daemon: &DaemonSettings,
        launch: Launch,
    ) -> Result<(Self, DaemonListener), ClaimError> {
        // Before the pidfile, which records how this process runs.
        let launchd = Launchd::check_in(daemon)?;
        let run_mode = match launchd {
            Some(_) => RunMode::LaunchdAgent,
            None => RunMode::detect(daemon, launch),
        };
        if let RunMode::External(Some(ExternalManager::LaunchdJob { label })) = &run_mode {
            Launchd::make_way_for(label).await;
        }

        // Only the pidfile's holder may clear the sockets a dead daemon left behind.
        let pidfile =
            Pidfile::new(&daemon.pidfile_path).acquire(&PidfileInfo::current(daemon, run_mode))?;
        let listening =
            launchd.unwrap_or_else(|| SocketHolder::configured(daemon)).listen(daemon)?;

        let claim = Self {
            #[cfg(unix)]
            _socket: listening.socket,
            _pidfile: pidfile,
        };
        Ok((claim, listening.listener))
    }
}
