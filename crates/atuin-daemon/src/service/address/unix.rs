use std::io;
use std::path::PathBuf;

use tokio::net::UnixStream;

/// A connection to the daemon.
pub type DaemonStream = UnixStream;

/// Where the daemon listens for clients: a Unix socket.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display, derive_more::From)]
#[display("{}", _0.display())]
pub struct DaemonAddress(PathBuf);

impl DaemonAddress {
    pub async fn connect(&self) -> io::Result<DaemonStream> {
        UnixStream::connect(&self.0).await
    }
}
