use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonAddress;
#[cfg(target_os = "linux")]
use crate::service::address::DaemonListener;
#[cfg(not(target_os = "linux"))]
use crate::service::backend::Subproc;
use crate::service::backend::{Backend, BackendError, Listening};
use crate::service::connection::Connection;

/// Represents a backend for a service that systemd manages, through a unit the user configured
/// rather than Atuin.
pub struct ExternalSystemd {
    address: DaemonAddress,
    daemon: DaemonSettings,
}

/// The error [`ExternalSystemd`] refuses to start or stop the service with.
#[derive(Debug, thiserror::Error)]
#[error(
    "the daemon is managed by systemd (`daemon.systemd_socket = true`); use `systemctl --user` to \
     start, stop or restart it"
)]
pub struct ManagedBySystemd;

/// Errors from taking the socket systemd passes the service it starts.
#[cfg(target_os = "linux")]
#[derive(Debug, thiserror::Error)]
pub enum SystemdSocketError {
    #[error("could not take the socket systemd passed in: {0}")]
    Take(std::io::Error),
    #[error("`daemon.systemd_socket` is set, but systemd passed in no socket")]
    Missing,
}

impl ExternalSystemd {
    #[must_use]
    pub fn new(daemon: &DaemonSettings) -> Self {
        Self {
            address: DaemonAddress::from(daemon.existing_socket_path().into_owned()),
            daemon: daemon.clone(),
        }
    }
}

impl Backend for ExternalSystemd {
    fn address(&self) -> DaemonAddress {
        self.address.clone()
    }

    fn locate(&self) -> Result<DaemonAddress, BackendError> {
        Ok(self.address.clone())
    }

    fn listen(&self) -> Result<Listening, BackendError> {
        #[cfg(target_os = "linux")]
        {
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
                    "could not detect systemd socket path, ensure that it's at the configured \
                     path: {:?}, error: {err:?}",
                    socket_path.as_path(),
                ),
            }

            Ok(Listening {
                listener: DaemonListener::from(listener),
                socket: None,
            })
        }

        // Only Linux has systemd; elsewhere, whatever runs the daemon leaves it to bind its own
        // socket.
        #[cfg(not(target_os = "linux"))]
        {
            Subproc::new(&self.daemon).listen()
        }
    }

    async fn launch(&self) -> Result<(), BackendError> {
        Err(BackendError::new(ManagedBySystemd))
    }

    async fn stop(&self, _service: &Connection) -> Result<(), BackendError> {
        Err(BackendError::new(ManagedBySystemd))
    }

    async fn evict(&self) -> Result<(), BackendError> {
        Err(BackendError::new(ManagedBySystemd))
    }
}
