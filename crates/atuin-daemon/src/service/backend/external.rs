use crate::service::address::DaemonAddress;
use crate::service::backend::{Backend, BackendError};
use crate::service::connection::Connection;

/// Represents a backend for a service that some external service (eg. `systemd` configured by the
/// user) manages.
pub struct External {
    address: DaemonAddress,
}

/// The error [`External`] refuses to start or stop the service with.
#[derive(Debug, thiserror::Error)]
#[error(
    "the daemon is managed by systemd (socket activation); use `systemctl --user` to start, stop \
     or restart it"
)]
pub struct ManagedBySystemd;

impl External {
    #[must_use]
    pub const fn new(address: DaemonAddress) -> Self {
        Self { address }
    }
}

impl Backend for External {
    fn address(&self) -> DaemonAddress {
        self.address.clone()
    }

    async fn launch(&self) -> Result<(), BackendError> {
        Err(BackendError::new(ManagedBySystemd))
    }

    async fn stop(&self, _service: &Connection) -> Result<(), BackendError> {
        Err(BackendError::new(ManagedBySystemd))
    }
}
