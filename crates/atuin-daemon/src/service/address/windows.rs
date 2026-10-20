use std::io;

use tokio::net::TcpStream;

/// A connection to the daemon.
pub type DaemonStream = TcpStream;

/// Where the daemon listens for clients: TCP on `127.0.0.1`, at this port.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display, derive_more::From)]
#[display("127.0.0.1:{_0}")]
pub struct DaemonAddress(u64);

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        TcpStream::connect(self.to_string()).await
    }
}
