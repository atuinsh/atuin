//! Where the daemon listens, and how a client connects to it there.
//!
//! The daemon listens on a Unix socket, or on Windows on TCP. Each platform's module defines the
//! same [`DaemonAddress`], [`DaemonListener`] and [`DaemonStream`].

#[cfg(unix)]
mod unix;
#[cfg(not(unix))]
mod windows;

use hyper_util::rt::TokioIo;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
#[cfg(unix)]
pub use unix::{DaemonAddress, DaemonListener, DaemonStream};
#[cfg(not(unix))]
pub use windows::{DaemonAddress, DaemonListener, DaemonStream};

impl DaemonAddress {
    /// Open a gRPC channel to the daemon at this address.
    pub async fn channel(&self) -> Result<Channel, ConnectError> {
        let address = self.clone();
        Endpoint::from_static("http://atuin_local_daemon:0")
            .connect_with_connector(service_fn(move |_: Uri| {
                let address = address.clone();
                async move { address.connect().await.map(TokioIo::new) }
            }))
            .await
            .map_err(|source| ConnectError {
                address: self.clone(),
                source,
            })
    }
}

/// Errors from connecting to the daemon.
#[derive(Debug, thiserror::Error)]
#[error("failed to connect to local atuin daemon at {address}. Is it running?")]
pub struct ConnectError {
    address: DaemonAddress,
    #[source]
    source: tonic::transport::Error,
}

impl ConnectError {
    /// Whether the connection failed because nothing listens at the address, rather than
    /// something refusing this client.
    pub(crate) fn nothing_listens(&self) -> bool {
        let source: &(dyn std::error::Error + 'static) = &self.source;
        std::iter::successors(Some(source), |err| err.source())
            .filter_map(|err| err.downcast_ref::<std::io::Error>())
            // `NotFound` is a missing socket, `ConnectionRefused` a stale one or an unbound port.
            .any(|err| {
                matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                )
            })
    }
}
