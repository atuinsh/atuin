#[cfg(unix)]
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
#[cfg(unix)]
use atuin_common::os::unix::SecureTempDirError;
#[cfg(unix)]
use daemonix::Daemonize;

use crate::service::address::{DaemonAddress, DaemonListener};
use crate::service::backend::{Backend, BackendError, Listening};
use crate::service::connection::Connection;
use crate::service::pidfile::Pidfile;
#[cfg(unix)]
use crate::service::pidfile::SocketPathError;
#[cfg(unix)]
use crate::service::socket_file::{RemoveSocketError, SocketFile};

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
    #[cfg(unix)]
    #[error(transparent)]
    SocketPath(#[from] SocketPathError),
    #[cfg(unix)]
    #[error(transparent)]
    RemoveSocket(#[from] RemoveSocketError),
    #[cfg(unix)]
    #[error("could not create the daemon's socket directory: {0}")]
    SocketDir(#[from] SecureTempDirError),
    #[error("could not listen on {address}: {source}")]
    Listen {
        address: DaemonAddress,
        source: std::io::Error,
    },
}

/// Errors from [`Subproc::daemonize`].
#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub enum DaemonizeError {
    #[error(transparent)]
    AtuinHome(#[from] atuin_common::dirs::HomeError),
    #[error("could not create the daemon's working directory {}: {source}", .path.display())]
    WorkingDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to daemonize process: {0}")]
    Daemonize(daemonix::Error),
}

impl Subproc {
    #[must_use]
    pub fn new(daemon: &DaemonSettings) -> Self {
        Self {
            daemon: daemon.clone(),
        }
    }

    /// Detach this process, as the service `launch` started.
    ///
    /// Call it before creating the async runtime or opening database connections, since `fork()`
    /// inside an async runtime corrupts its internal state.
    #[cfg(unix)]
    pub fn daemonize() -> Result<(), DaemonizeError> {
        let working_directory = PathBuf::from(atuin_common::dirs::try_atuin_home()?);

        // TODO(markovejnovic): We should have some sort of DirsProvider or something like that
        //                      which would guarantee that the directory would exist.
        std::fs::create_dir_all(&working_directory).map_err(|source| {
            DaemonizeError::WorkingDir {
                path: working_directory.clone(),
                source,
            }
        })?;

        Daemonize::new()
            .working_directory(working_directory)
            .umask(0o077)
            .start()
            .map_err(DaemonizeError::Daemonize)
    }

    fn bind(address: DaemonAddress) -> Result<DaemonListener, SubprocError> {
        match address.bind() {
            Ok(listener) => {
                tracing::info!("listening on {address}");
                Ok(listener)
            }
            Err(source) => Err(SubprocError::Listen { address, source }),
        }
    }
}

impl Backend for Subproc {
    fn address(&self) -> DaemonAddress {
        #[cfg(unix)]
        {
            DaemonAddress::from(self.daemon.socket_path().into_owned())
        }
        #[cfg(not(unix))]
        {
            DaemonAddress::from(self.daemon.tcp_port)
        }
    }

    fn locate(&self) -> Result<DaemonAddress, BackendError> {
        #[cfg(unix)]
        {
            let pidfile = Pidfile::new(&self.daemon.pidfile_path);
            let socket = pidfile.socket_path(&self.daemon).map_err(SubprocError::from);
            Ok(DaemonAddress::from(socket.map_err(BackendError::new)?))
        }
        #[cfg(not(unix))]
        {
            Ok(DaemonAddress::from(self.daemon.tcp_port))
        }
    }

    fn listen(&self) -> Result<Listening, BackendError> {
        #[cfg(unix)]
        {
            // A daemon that crashed left its socket behind, which would keep this one from listening.
            SocketFile::remove_stale(&self.daemon)
                .map_err(|err| BackendError::new(SubprocError::from(err)))?;

            let socket_path = self.daemon.socket_path();
            socket_path
                .create_default_dir_if_needed()
                .map_err(|err| BackendError::new(SubprocError::from(err)))?;
            let socket_path = socket_path.into_owned();
            let listener =
                Self::bind(DaemonAddress::from(socket_path.clone())).map_err(BackendError::new)?;

            Ok(Listening {
                listener,
                socket: Some(SocketFile::new(socket_path)),
            })
        }

        #[cfg(not(unix))]
        {
            let listener = Self::bind(self.address()).map_err(BackendError::new)?;
            Ok(Listening { listener })
        }
    }

    async fn launch(&self) -> Result<(), BackendError> {
        let exe = atuin_common::os::process::current_exe();

        let mut service = Command::new(exe);
        service
            .args(["daemon", "start"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(unix)]
        service.arg("--daemonize");

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
            if let Some(pid) = Pidfile::new(pidfile_path).pid()
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

        #[cfg(unix)]
        SocketFile::remove_all(&self.daemon, |_| true)
            .map_err(|err| BackendError::new(SubprocError::from(err)))?;

        Ok(())
    }
}
