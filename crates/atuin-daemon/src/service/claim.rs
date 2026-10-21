use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonListener;
#[cfg(target_os = "macos")]
use crate::service::backend::Launchd;
use crate::service::backend::{AnyBackend, Backend, BackendError};
use crate::service::pidfile::{AcquireGuardError, Pidfile, PidfileGuard, PidfileInfo};
#[cfg(target_os = "macos")]
use crate::service::run_mode::ExternalManager;
use crate::service::run_mode::{Launch, RunMode};
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
    /// Make this process, `launch`ed so, the service `daemon` describes, returning the listener to
    /// serve it on.
    ///
    /// The runtime it runs on keeps a socket it binds fresh until the claim is dropped.
    pub async fn take(
        daemon: &DaemonSettings,
        launch: Launch,
    ) -> Result<(Self, DaemonListener), ClaimError> {
        // Before the backend is picked: launchd starts the agent whichever backend a client would
        // now pick, as one does once another user holds the console. And before the pidfile, which
        // records how this process runs.
        #[cfg(target_os = "macos")]
        let check_in = Launchd::check_in(daemon)?;
        #[cfg(target_os = "macos")]
        let run_mode = match check_in {
            Some(_) => RunMode::LaunchdAgent,
            None => RunMode::detect(daemon, launch),
        };
        #[cfg(not(target_os = "macos"))]
        let run_mode = RunMode::detect(daemon, launch);

        #[cfg(target_os = "macos")]
        if let RunMode::External(Some(ExternalManager::LaunchdJob { label })) = &run_mode {
            Launchd::make_way_for(label).await;
        }

        // Only the pidfile's holder may clear the sockets a dead daemon left behind.
        let pidfile =
            Pidfile::new(&daemon.pidfile_path).acquire(&PidfileInfo::current(daemon, run_mode))?;
        // The pidfile now says how this process runs, and so which backend it listens as.
        #[cfg(target_os = "macos")]
        let listening = match check_in {
            Some(check_in) => check_in.listen(daemon),
            None => AnyBackend::new(daemon).listen(),
        }?;
        #[cfg(not(target_os = "macos"))]
        let listening = AnyBackend::new(daemon).listen()?;

        let claim = Self {
            #[cfg(unix)]
            _socket: listening.socket,
            _pidfile: pidfile,
        };
        Ok((claim, listening.listener))
    }
}
