use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonAddress;
use crate::service::backend::{Backend, BackendError};
use crate::service::connection::Connection;

/// Represents a backend for a service that systemd manages, through a unit the user configured
/// rather than Atuin.
pub struct ExternalSystemd {
    address: DaemonAddress,
}

/// The error [`ExternalSystemd`] refuses to start or stop the service with.
#[derive(Debug, thiserror::Error)]
#[error(
    "the daemon is managed by systemd (`daemon.systemd_socket = true`); use `systemctl --user` to \
     start, stop or restart it"
)]
pub struct ManagedBySystemd;

impl ExternalSystemd {
    #[must_use]
    pub fn new(daemon: &DaemonSettings) -> Self {
        Self {
            address: DaemonAddress::existing(daemon),
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
