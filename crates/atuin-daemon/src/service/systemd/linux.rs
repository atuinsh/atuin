use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonListener;
use crate::service::socket_holder::{ListenError, Listening};

/// systemd, the service manager.
pub enum Systemd {}

impl Systemd {
    /// The socket systemd passed this process, as `daemon.systemd_socket` says it does.
    pub(in crate::service) fn take_socket(
        daemon: &DaemonSettings,
    ) -> Result<Listening, ListenError> {
        tracing::info!("getting systemd socket");
        let listener = listenfd::ListenFd::from_env()
            .take_unix_listener(0)
            .map_err(ListenError::TakeSystemd)?
            .ok_or(ListenError::NoSystemdSocket)?;

        let socket_path = daemon.socket_path();
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
