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

    /// Kill the daemon holding the pidfile, if one does, leaving behind what it doesn't remove as
    /// it exits.
    pub(in crate::service) async fn terminate(&self) {
        const GRACE: Duration = Duration::from_secs(2);

        let pidfile = Pidfile::new(&self.daemon.pidfile_path);
        // The PID of a pidfile nobody holds may since have gone to another process.
        if pidfile.is_live()
            && let Some(pid) = pidfile.pid()
            && pid != 0
            && let Err(e) = atuin_common::os::process::force_terminate(pid, GRACE).await
        {
            tracing::warn!("could not terminate existing daemon (pid {pid}): {e}");
        }
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
        let pidfile_path = self.daemon.pidfile_path.as_path();
        if pidfile_path.exists() {
            self.terminate().await;

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

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;

    use super::*;
    use crate::service::RunMode;
    use crate::service::pidfile::PidfileInfo;

    /// A pidfile that no daemon holds names a PID that may since have gone to another process,
    /// here this test's own: had evicting signalled it, the test would have died.
    #[rstest]
    #[tokio::test]
    async fn evicting_spares_the_pid_of_a_stale_pidfile() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.daemon.pidfile_path =
            AbsPathBuf::new(dir.path().join("atuin-daemon.pid")).unwrap();
        // Evicting removes the daemon's socket wherever it may be, which would be the real one.
        settings.daemon.socket_path = AbsPathBuf::new(dir.path().join("atuin.sock")).ok();
        let info = PidfileInfo {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            socket_path: None,
            run_mode: Some(RunMode::DaemonizeForked),
            program: None,
        };
        drop(Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap());

        Subproc::new(&settings.daemon).evict().await.unwrap();

        assert!(!settings.daemon.pidfile_path.exists(), "the stale pidfile is still there");
    }
}
