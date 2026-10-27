mod locate;
mod socket_file;

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::os::unix::SecureTempDirError;
pub use locate::LocateError;
pub use socket_file::{RemoveSocketError, SocketFile};
use tokio::net::UnixStream;
use tokio_stream::wrappers::UnixListenerStream;

/// A connection to the daemon.
pub type DaemonStream = UnixStream;

/// Where the daemon listens for clients: a Unix socket.
#[derive(
    Clone, Debug, PartialEq, Eq, derive_more::AsRef, derive_more::Display, derive_more::From,
)]
#[as_ref(Path)]
#[display("{}", _0.display())]
pub struct DaemonAddress(PathBuf);

/// The socket the daemon accepts clients on.
#[derive(Debug, derive_more::From)]
pub struct DaemonListener(std::os::unix::net::UnixListener);

/// Errors from clearing the daemon's socket and creating its directory, ready to bind it.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error(transparent)]
    RemoveSocket(#[from] RemoveSocketError),
    #[error("could not create the daemon's socket directory: {0}")]
    SocketDir(#[from] SecureTempDirError),
}

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        UnixStream::connect(&self.0).await
    }

    pub fn bind(&self) -> io::Result<DaemonListener> {
        std::os::unix::net::UnixListener::bind(&self.0).map(DaemonListener)
    }

    /// Where the daemon `daemon` describes binds: its configured socket.
    #[must_use]
    pub fn configured(daemon: &DaemonSettings) -> Self {
        Self(daemon.socket_path().into_owned())
    }

    /// Where a running daemon `daemon` describes listens, going by the sockets that exist, when
    /// its pidfile doesn't say.
    #[must_use]
    pub fn existing(daemon: &DaemonSettings) -> Self {
        Self(daemon.existing_socket_path().into_owned())
    }

    /// The socket's file, for the pidfile to record.
    #[must_use]
    pub fn into_socket_path(self) -> Option<PathBuf> {
        Some(self.0)
    }

    /// The configured socket, cleared of the sockets a daemon that crashed left behind and with
    /// its default directory created, ready to bind. Only the pidfile's holder may clear them.
    pub(crate) fn prepare(daemon: &DaemonSettings) -> Result<Self, PrepareError> {
        // A stale socket would keep this daemon from binding.
        SocketFile::remove_stale(daemon)?;
        let socket_path = daemon.socket_path();
        socket_path.create_default_dir_if_needed()?;
        Ok(Self(socket_path.into_owned()))
    }

    /// The paths at which a daemon `daemon` describes may have left its socket.
    ///
    /// For the same reasons explained in [`Self::locate`], when `ATUIN_HOME` is set, this only
    /// yields the [preferred socket path][0] instead of also including the legacy paths.
    ///
    /// [0]: atuin_client::settings::Daemon::preferred_socket_path
    fn candidates(daemon: &DaemonSettings) -> impl Iterator<Item = Cow<'_, Path>> + use<'_> {
        if atuin_common::dirs::atuin_home_is_set() {
            itertools::Either::Left(std::iter::once(daemon.preferred_socket_path()))
        } else {
            itertools::Either::Right(daemon.potential_socket_paths())
        }
    }
}

impl DaemonListener {
    /// The clients connecting to the daemon, accepted on the current Tokio runtime.
    pub fn into_incoming(self) -> io::Result<UnixListenerStream> {
        self.0.set_nonblocking(true)?;
        Ok(UnixListenerStream::new(tokio::net::UnixListener::from_std(self.0)?))
    }
}
