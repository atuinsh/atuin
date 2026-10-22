use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::backend::ExternalRestartError;
use crate::service::socket_holder::{ListenError, Listening, SocketHolder};

/// systemd, which there is none of off Linux.
pub enum Systemd {}

impl Systemd {
    /// Only Linux has systemd; elsewhere, the daemon binds its own socket.
    pub(in crate::service) fn take_socket(
        daemon: &DaemonSettings,
    ) -> Result<Listening, ListenError> {
        SocketHolder::Daemon.listen(daemon)
    }

    pub(in crate::service) const fn unit_of_this_process() -> Option<String> {
        None
    }

    pub(in crate::service) fn restart_unit(
        _unit: &str,
    ) -> impl Future<Output = Result<(), ExternalRestartError>> {
        std::future::ready(Err(ExternalRestartError::Missing { manager: "systemd" }))
    }
}
