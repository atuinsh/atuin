//! Client for the daemon's `octavo.Threads` gRPC service.

use atuin_client::ai_session::HarnessSession;
use atuin_client::settings::Settings;
use tonic::Code;

use super::{DaemonChannel, FromSettingsError};
use crate::grpc::octavo::pb::threads_client::ThreadsClient as ThreadsServiceClient;
use crate::grpc::octavo::pb::{ThreadRequest, ThreadShares};
use crate::grpc::request::ClientInterceptor;
use crate::service::ActiveService;
use crate::service::address::{ConnectError, DaemonAddress};

/// Client for the daemon's `octavo.Threads` service.
pub struct ThreadsClient {
    client: ThreadsServiceClient<DaemonChannel>,
}

impl ThreadsClient {
    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        let channel = address.into().channel().await?;
        Ok(Self {
            client: ThreadsServiceClient::with_interceptor(channel, ClientInterceptor),
        })
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
    }

    /// Get Octavo sharing info for `thread`, or [`None`] if the daemon was built without Octavo.
    pub async fn shares(
        &mut self,
        thread: HarnessSession,
    ) -> Result<Option<ThreadShares>, tonic::Status> {
        built_with_octavo(self.client.get_shares(thread_request(thread)).await)
    }

    /// Make `thread` private and get its sharing info afterwards, or [`None`] if the daemon was
    /// built without Octavo.
    pub async fn make_private(
        &mut self,
        thread: HarnessSession,
    ) -> Result<Option<ThreadShares>, tonic::Status> {
        built_with_octavo(self.client.make_private(thread_request(thread)).await)
    }
}

fn thread_request(thread: HarnessSession) -> ThreadRequest {
    ThreadRequest {
        thread: Some(thread.into()),
    }
}

/// Unwrap an RPC response, mapping `Unimplemented` to [`None`] when Octavo is compiled out.
fn built_with_octavo<T>(
    reply: Result<tonic::Response<T>, tonic::Status>,
) -> Result<Option<T>, tonic::Status> {
    match reply {
        Ok(reply) => Ok(Some(reply.into_inner())),
        Err(status) if status.code() == Code::Unimplemented => Ok(None),
        Err(status) => Err(status),
    }
}
