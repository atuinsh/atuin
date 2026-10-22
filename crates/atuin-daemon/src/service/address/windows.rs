use std::io;
use std::path::PathBuf;

use atuin_client::settings::Daemon as DaemonSettings;
use tokio::net::TcpStream;
use tokio_stream::wrappers::TcpListenerStream;

use crate::service::pidfile::Pidfile;

/// A connection to the daemon.
pub type DaemonStream = TcpStream;

/// Where the daemon listens for clients: TCP on `127.0.0.1`, at this port.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display, derive_more::From)]
#[display("127.0.0.1:{_0}")]
pub struct DaemonAddress(u64);

/// The socket the daemon accepts clients on.
#[derive(Debug)]
pub struct DaemonListener(std::net::TcpListener);

/// The socket file a daemon listens on, which there is none of over TCP.
pub enum SocketFile {}

/// Errors from removing a socket file, which there is none of over TCP.
#[derive(Debug, thiserror::Error)]
pub enum RemoveSocketError {}

/// Errors from getting ready to bind, which over TCP has nothing to clear.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {}

/// Errors from [`DaemonAddress::locate`], which always finds the configured port.
#[derive(Debug, thiserror::Error)]
pub enum LocateError {}

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        TcpStream::connect(self.to_string()).await
    }

    pub fn bind(&self) -> io::Result<DaemonListener> {
        std::net::TcpListener::bind(self.to_string()).map(DaemonListener)
    }

    /// Where the daemon `daemon` describes binds: its configured port.
    #[must_use]
    pub const fn configured(daemon: &DaemonSettings) -> Self {
        Self(daemon.tcp_port)
    }

    /// Where a running daemon `daemon` describes listens: its configured port.
    #[must_use]
    pub const fn existing(daemon: &DaemonSettings) -> Self {
        Self::configured(daemon)
    }

    /// Where the running daemon `daemon` describes listens: its configured port, whatever its
    /// pidfile says.
    #[must_use]
    pub const fn locate(daemon: &DaemonSettings, _pidfile: &Pidfile) -> Result<Self, LocateError> {
        Ok(Self::configured(daemon))
    }

    /// No file, for the pidfile to record.
    #[must_use]
    pub const fn into_socket_path(self) -> Option<PathBuf> {
        None
    }

    /// The configured port, which nothing has to be cleared from to bind.
    #[expect(clippy::unnecessary_wraps, reason = "clearing a Unix socket can fail")]
    pub(crate) const fn prepare(daemon: &DaemonSettings) -> Result<Self, PrepareError> {
        Ok(Self::configured(daemon))
    }
}

impl DaemonListener {
    /// The clients connecting to the daemon, accepted on the current Tokio runtime.
    pub fn into_incoming(self) -> io::Result<TcpListenerStream> {
        self.0.set_nonblocking(true)?;
        Ok(TcpListenerStream::new(tokio::net::TcpListener::from_std(self.0)?))
    }
}

impl SocketFile {
    pub(crate) const fn of(_address: &DaemonAddress) -> Option<Self> {
        None
    }

    #[expect(clippy::unnecessary_wraps, reason = "removing a Unix socket can fail")]
    pub(crate) const fn remove_all(_daemon: &DaemonSettings) -> Result<(), RemoveSocketError> {
        Ok(())
    }
}
