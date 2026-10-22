use atuin_client::settings::Daemon as DaemonSettings;

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
}
