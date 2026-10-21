use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonAddress;
#[cfg(target_os = "linux")]
use crate::service::address::DaemonListener;
use crate::service::backend::{Backend, BackendError, Listening, Subproc};
use crate::service::connection::Connection;
use crate::service::run_mode::ExternalManager;

/// Represents a backend for a service that something other than Atuin runs, through a unit or job
/// the user set up.
pub struct External {
    address: DaemonAddress,
    daemon: DaemonSettings,
    manager: ExternalManager,
}

/// Errors from taking the socket systemd passes the service it starts.
#[cfg(target_os = "linux")]
#[derive(Debug, thiserror::Error)]
pub enum SystemdSocketError {
    #[error("could not take the socket systemd passed in: {0}")]
    Take(std::io::Error),
    #[error("`daemon.systemd_socket` is set, but systemd passed in no socket")]
    Missing,
}

impl External {
    #[must_use]
    pub fn new(daemon: &DaemonSettings, manager: ExternalManager) -> Self {
        Self {
            address: DaemonAddress::from(daemon.existing_socket_path().into_owned()),
            daemon: daemon.clone(),
            manager,
        }
    }

    #[cfg(target_os = "linux")]
    fn take_systemd_socket(&self) -> Result<Listening, BackendError> {
        tracing::info!("getting systemd socket");
        let listener = listenfd::ListenFd::from_env()
            .take_unix_listener(0)
            .map_err(|err| BackendError::new(SystemdSocketError::Take(err)))?
            .ok_or_else(|| BackendError::new(SystemdSocketError::Missing))?;

        let socket_path = self.daemon.socket_path();
        match listener.local_addr() {
            Ok(addr) => match addr.as_pathname() {
                Some(actual_path) => {
                    tracing::info!("listening on systemd socket: {actual_path:?}");
                    if actual_path != socket_path.as_path() {
                        tracing::warn!(
                            "systemd socket is not at configured client path: {:?}",
                            socket_path.as_path(),
                        );
                    }
                }
                None => tracing::warn!(
                    "could not detect systemd socket path, ensure that it's at the configured \
                     path: {:?}, error: systemd socket missing path",
                    socket_path.as_path(),
                ),
            },
            Err(err) => tracing::warn!(
                "could not detect systemd socket path, ensure that it's at the configured path: \
                 {:?}, error: {err:?}",
                socket_path.as_path(),
            ),
        }

        Ok(Listening {
            listener: DaemonListener::from(listener),
            socket: None,
        })
    }
}

impl Backend for External {
    fn address(&self) -> DaemonAddress {
        self.address.clone()
    }

    fn listen(&self) -> Result<Listening, BackendError> {
        match &self.manager {
            #[cfg(target_os = "linux")]
            ExternalManager::SystemdSocket => self.take_systemd_socket(),
            // Only Linux has systemd, and every other manager leaves the daemon to bind its own
            // socket.
            #[cfg(not(target_os = "linux"))]
            ExternalManager::SystemdSocket => Subproc::new(&self.daemon).listen(),
            ExternalManager::SystemdUnit { .. } | ExternalManager::LaunchdJob { .. } => {
                Subproc::new(&self.daemon).listen()
            }
        }
    }

    async fn launch(&self) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }

    async fn stop(&self, _service: &Connection) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }

    async fn evict(&self) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }
}
