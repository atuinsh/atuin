use std::io;
#[cfg(target_os = "macos")]
use std::os::unix::net::UnixListener;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use atuin_client::settings::Daemon as DaemonSettings;
#[cfg(unix)]
use atuin_common::os::unix::SecureTempDirError;

use crate::service::address::{DaemonAddress, DaemonListener};
#[cfg(target_os = "macos")]
use crate::service::backend::Launchd;
#[cfg(unix)]
use crate::service::socket_file::{RemoveSocketError, SocketFile};

/// Who holds the socket the daemon serves on: the daemon itself, or launchd or systemd, which keep
/// listening on it while no daemon runs.
pub(super) enum SocketHolder {
    /// The daemon, which binds the configured socket itself and removes it as it exits.
    Daemon,
    /// launchd, which checked Atuin's agent in with the socket it holds for it.
    #[cfg(target_os = "macos")]
    Launchd(UnixListener),
    /// systemd, which passed the socket in, as `daemon.systemd_socket` says it does.
    #[cfg(target_os = "linux")]
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

/// Errors from [`SocketHolder::listen`].
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
    #[cfg(target_os = "macos")]
    #[error(
        "launchd holds {} for Atuin's login item. Stop the login item with `atuin daemon stop` \
         first, or use `atuin daemon start --force`",
        .0.display()
    )]
    HeldByLaunchd(PathBuf),
    #[cfg(target_os = "linux")]
    #[error("could not take the socket systemd passed in: {0}")]
    TakeSystemd(io::Error),
    #[cfg(target_os = "linux")]
    #[error("`daemon.systemd_socket` is set, but systemd passed in no socket")]
    NoSystemdSocket,
}

impl SocketHolder {
    /// Who holds the socket of the daemon `daemon` describes, when launchd didn't check it in as
    /// Atuin's agent.
    pub(super) fn configured(daemon: &DaemonSettings) -> Self {
        // Only Linux has systemd.
        #[cfg(target_os = "linux")]
        if daemon.systemd_socket {
            return Self::Systemd;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = daemon;

        Self::Daemon
    }

    /// Take up the socket to serve on. Binding one clears the sockets a dead daemon left behind,
    /// which only the daemon holding the pidfile may.
    pub(super) fn listen(self, daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        match self {
            Self::Daemon => Self::bind(daemon),
            #[cfg(target_os = "macos")]
            Self::Launchd(listener) => {
                let socket_path = daemon.socket_path().into_owned();
                tracing::info!("listening on launchd socket {}", socket_path.display());
                Ok(Listening {
                    listener: DaemonListener::from(listener),
                    socket: Some(SocketFile::held_by_launchd(socket_path)),
                })
            }
            #[cfg(target_os = "linux")]
            Self::Systemd => Self::take_systemd_socket(daemon),
        }
    }

    #[cfg(unix)]
    fn bind(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        // A daemon that crashed left its socket behind, which would keep this one from listening.
        SocketFile::remove_stale(daemon)?;

        let socket_path = daemon.socket_path();
        socket_path.create_default_dir_if_needed()?;
        let socket_path = socket_path.into_owned();
        let address = DaemonAddress::from(socket_path.clone());
        let listener = match address.bind() {
            Ok(listener) => listener,
            // launchd listens on the agent's socket for as long as the agent is installed.
            #[cfg(target_os = "macos")]
            Err(source)
                if source.kind() == io::ErrorKind::AddrInUse
                    && Launchd::serves_this_profile()
                    && Launchd::is_installed() =>
            {
                return Err(ListenError::HeldByLaunchd(socket_path));
            }
            Err(source) => return Err(ListenError::Bind { address, source }),
        };
        tracing::info!("listening on {address}");

        Ok(Listening {
            listener,
            socket: Some(SocketFile::new(socket_path)),
        })
    }

    #[cfg(not(unix))]
    fn bind(daemon: &DaemonSettings) -> Result<Listening, ListenError> {
        let address = DaemonAddress::from(daemon.tcp_port);
        let listener = match address.bind() {
            Ok(listener) => listener,
            Err(source) => return Err(ListenError::Bind { address, source }),
        };
        tracing::info!("listening on {address}");

        Ok(Listening { listener })
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
}
