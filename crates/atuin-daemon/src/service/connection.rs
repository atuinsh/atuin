use std::ops::ControlFlow;
use std::time::Duration;

use atuin_common::futures::Backoff;
use tonic::Code;
use tonic::transport::Channel;

use crate::HistoryClient;
use crate::client::LogListener;
use crate::grpc::versioning::VersionMatchError;
use crate::service::address::{ConnectError, DaemonAddress};
use crate::service::backend::{BackendError, SpawnError};

/// A connection to a running service, whatever its build.
pub struct Connection {
    channel: Channel,
    address: DaemonAddress,
    /// How the service's build compared with this client's when it was found.
    build: Result<(), VersionMatchError>,
}

/// Errors from finding a service.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    #[error(transparent)]
    Locate(#[from] BackendError),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("legacy daemon detected; restart daemon manually")]
    Legacy,
    #[error("the daemon accepted the connection but didn't answer within {}ms", .0.as_millis())]
    Unresponsive(Duration),
    #[error("could not ask the daemon for its status: {0}")]
    Status(Box<tonic::Status>),
}

impl Connection {
    /// How often to check whether a just-launched service answers yet.
    const READY_POLL: Duration = Duration::from_millis(40);

    /// The service answering at `address`, if any, whatever its build.
    pub(super) async fn at(address: DaemonAddress) -> Result<Option<Self>, FindError> {
        let channel = match address.channel().await {
            Ok(channel) => channel,
            Err(err) if err.nothing_listens() => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        match HistoryClient::from(channel.clone()).check_daemon_version().await {
            Ok(build) => Ok(Some(Self {
                channel,
                address,
                build,
            })),
            Err(status) if status.code() == Code::Unavailable => Ok(None),
            Err(status) if status.code() == Code::Unimplemented => Err(FindError::Legacy),
            Err(status) => Err(FindError::Status(Box::new(status))),
        }
    }

    /// Wait for a just-launched service of this client's build to answer where `locate` says it
    /// listens, past one of another build that is still on its way out.
    pub(super) async fn connect_when_ready(
        locate: impl Fn() -> Result<DaemonAddress, FindError>,
        timeout: Duration,
    ) -> Result<Self, SpawnError> {
        let locate = &locate;
        let poll = Backoff::Constant(Self::READY_POLL).retry(
            || async move {
                let address = match locate() {
                    Ok(address) => address,
                    Err(err) => return ControlFlow::Break(Err(err.into())),
                };
                match Self::at(address).await {
                    Ok(Some(connection)) => match connection.ensure_our_build() {
                        Ok(()) => ControlFlow::Break(Ok(connection)),
                        Err(other) => ControlFlow::Continue(Some(other)),
                    },
                    Ok(None) => ControlFlow::Continue(None),
                    Err(err) => ControlFlow::Break(Err(err.into())),
                }
            },
            timeout,
        );

        // `retry` doesn't bound its first poll, which waits on a service that accepts the connection
        // but has yet to answer.
        match tokio::time::timeout(timeout, poll).await {
            Ok(Ok(answered)) => answered,
            Ok(Err(Some(other))) => Err(SpawnError::Mismatch(other)),
            Ok(Err(None)) | Err(_) => Err(SpawnError::NotReady(timeout)),
        }
    }

    pub const fn address(&self) -> &DaemonAddress {
        &self.address
    }

    /// Check that the service is this client's build.
    pub fn ensure_our_build(&self) -> Result<(), VersionMatchError> {
        self.build.clone()
    }

    /// A client of the service's history API, over this connection.
    pub fn history_client(&self) -> HistoryClient {
        HistoryClient::from(self.channel.clone())
    }

    /// Starts showing what the service logs for the user, over this connection.
    pub(crate) async fn listen_logs(&self) -> Result<LogListener, tonic::Status> {
        LogListener::listen(self.channel.clone()).await
    }

    /// Ask the service to shut down, without waiting for it to.
    ///
    /// Best effort: a service that is already gone, or doesn't take the request, is the business
    /// of whoever then waits for it to exit.
    pub(super) async fn request_shutdown(&self) {
        let _ = self.history_client().shutdown().await;
    }
}
