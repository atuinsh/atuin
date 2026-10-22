use std::io;

use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::{DaemonAddress, DaemonListener, PrepareError, SocketFile};
use crate::service::systemd::Systemd;

/// Who holds the socket the daemon serves on: the daemon itself, or systemd, which keeps listening
/// on it while no daemon runs.
pub(super) enum SocketHolder {
    /// The daemon, which binds the configured socket itself and removes it as it exits.
    Daemon,
    /// systemd, which passes the socket in, as `daemon.systemd_socket` says it does.
    Systemd,
}

/// The socket the daemon serves on.
pub(super) struct Listening {
    pub listener: DaemonListener,
    /// The socket file the daemon bound itself, which it owns until it exits.
    pub socket: Option<SocketFile>,
}

/// Errors from taking up the socket the daemon serves on.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[error(transparent)]
    Prepare(#[from] PrepareError),
    #[error("could not listen on {address}: {source}")]
    Bind {
        address: DaemonAddress,
        source: io::Error,
    },
    #[error("could not take the socket systemd passed in: {0}")]
    TakeSystemd(io::Error),
    #[error("`daemon.systemd_socket` is set, but systemd passed in no socket")]
    NoSystemdSocket,
}

impl SocketHolder {
    /// Who holds the socket of the daemon `daemon` describes.
    pub(super) const fn configured(daemon: &DaemonSettings) -> Self {
        if daemon.systemd_socket {
            Self::Systemd
        } else {
            Self::Daemon
        }
    }

    /// Take up the socket to serve on. Binding one clears the sockets a dead daemon left behind,
    /// which only the daemon holding the pidfile may.
    pub(super) fn listen(self, daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        match self {
            Self::Daemon => Self::bind(daemon),
            Self::Systemd => Systemd::take_socket(daemon),
        }
    }

    fn bind(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        let address = DaemonAddress::prepare(daemon)?;
        let listener = match address.bind() {
            Ok(listener) => listener,
            Err(source) => return Err(ListenError::Bind { address, source }),
        };
        tracing::info!("listening on {address}");

        Ok(Listening {
            socket: SocketFile::of(&address),
            listener,
        })
    }
}
