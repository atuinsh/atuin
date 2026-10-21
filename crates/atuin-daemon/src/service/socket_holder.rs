use std::io;

use atuin_client::settings::Daemon as DaemonSettings;
#[cfg(unix)]
use atuin_common::os::unix::SecureTempDirError;

use crate::service::address::{DaemonAddress, DaemonListener};
use crate::service::backend::Launchd;
#[cfg(unix)]
use crate::service::socket_file::{RemoveSocketError, SocketFile};

/// Who holds the socket the daemon serves on: the daemon itself, or launchd or systemd, which keep
/// listening on it while no daemon runs.
pub(super) enum SocketHolder {
    /// The daemon, which binds the configured socket itself and removes it as it exits.
    Daemon,
    /// launchd, which handed Atuin's agent the socket it holds for it.
    #[allow(dead_code, reason = "only macOS has launchd")]
    Launchd(Listening),
    /// systemd, which passes the socket in, as `daemon.systemd_socket` says it does.
    Systemd,
}

/// The socket the daemon serves on.
pub(super) struct Listening {
    pub listener: DaemonListener,
    /// The socket file the daemon listens on, kept fresh while it runs and, if it bound the file
    /// itself, removed when it exits.
    #[cfg(unix)]
    pub socket: Option<SocketFile>,
}

/// Errors from taking up the socket the daemon serves on.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[cfg(unix)]
    #[error(transparent)]
    RemoveSocket(#[from] RemoveSocketError),
    #[cfg(unix)]
    #[error("could not create the daemon's socket directory: {0}")]
    SocketDir(#[from] SecureTempDirError),
    #[error("could not listen on {address}: {source}")]
    Bind {
        address: DaemonAddress,
        source: io::Error,
    },
    #[error(
        "launchd holds {0} for Atuin's login item. Stop the login item with `atuin daemon stop` \
         first, or use `atuin daemon start --force`"
    )]
    HeldByLaunchd(DaemonAddress),
    #[error("could not take the socket systemd passed in: {0}")]
    TakeSystemd(io::Error),
    #[error("`daemon.systemd_socket` is set, but systemd passed in no socket")]
    NoSystemdSocket,
}

impl SocketHolder {
    /// Who holds the socket of the daemon `daemon` describes, when launchd didn't check it in as
    /// Atuin's agent.
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
            Self::Launchd(listening) => Ok(listening),
            Self::Systemd => Self::take_systemd_socket(daemon),
        }
    }

    fn bind(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        #[cfg(unix)]
        let (address, socket_path) = {
            // A daemon that crashed left its socket behind, which would keep this one from
            // listening.
            SocketFile::remove_stale(crate::client::potential_socket_paths(daemon))?;
            let socket_path = daemon.socket_path();
            socket_path.create_default_dir_if_needed()?;
            let socket_path = socket_path.into_owned();
            (DaemonAddress::from(socket_path.clone()), socket_path)
        };
        #[cfg(not(unix))]
        let address = DaemonAddress::from(daemon.tcp_port);

        let listener = match address.bind() {
            Ok(listener) => listener,
            Err(source) if source.kind() == io::ErrorKind::AddrInUse && Launchd::holds_socket() => {
                return Err(ListenError::HeldByLaunchd(address));
            }
            Err(source) => return Err(ListenError::Bind { address, source }),
        };
        tracing::info!("listening on {address}");

        Ok(Listening {
            listener,
            #[cfg(unix)]
            socket: Some(SocketFile::new(socket_path)),
        })
    }

    #[cfg(target_os = "linux")]
    fn take_systemd_socket(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
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

    /// Only Linux has systemd; elsewhere, the daemon binds its own socket.
    #[cfg(not(target_os = "linux"))]
    fn take_systemd_socket(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        Self::bind(daemon)
    }
}
