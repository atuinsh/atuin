use std::io;

use tokio::net::TcpStream;
use tokio_stream::wrappers::TcpListenerStream;

/// A connection to the daemon.
pub type DaemonStream = TcpStream;

/// Where the daemon listens for clients: TCP on `127.0.0.1`, at this port.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display, derive_more::From)]
#[display("127.0.0.1:{_0}")]
pub struct DaemonAddress(u64);

/// The socket the daemon accepts clients on.
#[derive(Debug)]
pub struct DaemonListener(std::net::TcpListener);

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        TcpStream::connect(self.to_string()).await
    }

    pub fn bind(&self) -> io::Result<DaemonListener> {
        std::net::TcpListener::bind(self.to_string()).map(DaemonListener)
    }
}

impl DaemonListener {
    /// The clients connecting to the daemon, accepted on the current Tokio runtime.
    pub fn into_incoming(self) -> io::Result<TcpListenerStream> {
        self.0.set_nonblocking(true)?;
        Ok(TcpListenerStream::new(tokio::net::TcpListener::from_std(self.0)?))
    }
}
