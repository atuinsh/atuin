use std::io;
use std::path::PathBuf;

use tokio::net::UnixStream;
use tokio_stream::wrappers::UnixListenerStream;

/// A connection to the daemon.
pub type DaemonStream = UnixStream;

/// Where the daemon listens for clients: a Unix socket.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display, derive_more::From)]
#[display("{}", _0.display())]
pub struct DaemonAddress(PathBuf);

/// The socket the daemon accepts clients on.
#[derive(Debug, derive_more::From)]
pub struct DaemonListener(std::os::unix::net::UnixListener);

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        UnixStream::connect(&self.0).await
    }

    pub fn bind(&self) -> io::Result<DaemonListener> {
        std::os::unix::net::UnixListener::bind(&self.0).map(DaemonListener)
    }
}

impl DaemonListener {
    /// The clients connecting to the daemon, accepted on the current Tokio runtime.
    pub fn into_incoming(self) -> io::Result<UnixListenerStream> {
        self.0.set_nonblocking(true)?;
        Ok(UnixListenerStream::new(tokio::net::UnixListener::from_std(self.0)?))
    }
}
