//! Forking off the client, as Unix does; on Windows, a spawned process already outlives its
//! client. Each platform's module defines the same names.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(not(unix))]
mod windows;
use std::process::{Command, Stdio};
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
pub use platform::DaemonizeError;
#[cfg(not(unix))]
use windows as platform;

use crate::service::address::{DaemonAddress, LocateError, RemoveSocketError, SocketFile};
use crate::service::backend::{Backend, BackendError};
use crate::service::connection::Connection;
use crate::service::pidfile::Pidfile;

/// Represents a backend that manages atuin-daemon as a background task by forking a user's regular
/// `atuin` process.
pub struct Subproc {
    daemon: DaemonSettings,
}

/// Errors from [`Subproc`]'s [`Backend`] methods.
#[derive(Debug, thiserror::Error)]
pub enum SubprocError {
    #[error("failed to launch the daemon: {0}")]
    Launch(std::io::Error),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    RemoveSocket(#[from] RemoveSocketError),
}

impl Subproc {
    #[must_use]
    pub fn new(daemon: &DaemonSettings) -> Self {
        Self {
            daemon: daemon.clone(),
        }
    }

    /// Where the running service listens, as its pidfile says.
    pub(super) fn locate(&self) -> Result<DaemonAddress, BackendError> {
        DaemonAddress::locate(&self.daemon, &Pidfile::new(&self.daemon.pidfile_path))
            .map_err(|err| BackendError::new(SubprocError::from(err)))
    }
}

impl Backend for Subproc {
    fn address(&self) -> DaemonAddress {
        DaemonAddress::configured(&self.daemon)
    }

    async fn launch(&self) -> Result<(), BackendError> {
        let exe = atuin_common::os::process::current_exe();

        let mut service = Command::new(exe);
        service
            .args(["daemon", "start", "--daemonize"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        service.spawn().map_err(|err| BackendError::new(SubprocError::Launch(err)))?;
        Ok(())
    }

    async fn stop(&self, service: &Connection) -> Result<(), BackendError> {
        service.request_shutdown().await;
        Ok(())
    }

    async fn evict(&self) -> Result<(), BackendError> {
        const GRACE: Duration = Duration::from_secs(2);

        let pidfile_path = self.daemon.pidfile_path.as_path();
        if pidfile_path.exists() {
            let pidfile = Pidfile::new(pidfile_path);
            // The PID of a pidfile nobody holds may since have gone to another process.
            if pidfile.is_live()
                && let Some(pid) = pidfile.pid()
                && pid != 0
                && let Err(e) = atuin_common::os::process::force_terminate(pid, GRACE).await
            {
                tracing::warn!("could not terminate existing daemon (pid {pid}): {e}");
            }

            if let Err(e) = std::fs::remove_file(pidfile_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("failed to remove pidfile: {e}");
            }
        }

        SocketFile::remove_all(&self.daemon)
            .map_err(|err| BackendError::new(SubprocError::from(err)))?;

        Ok(())
    }
}
