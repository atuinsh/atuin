//! Client for the daemon's `octavo.Threads` gRPC service.

use std::path::Path;

use atuin_client::ai_session::HarnessSession;
use atuin_client::settings::Settings;
use tonic::Code;

use super::{DaemonChannel, FromSettingsError};
use crate::grpc::octavo::pb::threads_client::ThreadsClient as ThreadsServiceClient;
use crate::grpc::octavo::pb::{
    AddLabelsRequest, NewThreadRequest, ThreadRequest, ThreadShares, ThreadStartedRequest,
};
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

    /// Get a link on the hub's web app that opens `thread` to anyone holding it.
    pub async fn share_link(&mut self, thread: HarnessSession) -> Result<String, tonic::Status> {
        Ok(self.client.share_link(thread_request(thread)).await?.into_inner().url)
    }

    /// Add the labels named `names` to `thread` and return the names of all its labels.
    pub async fn add_labels(
        &mut self,
        thread: HarnessSession,
        names: Vec<String>,
    ) -> Result<Vec<String>, tonic::Status> {
        let request = AddLabelsRequest {
            thread: Some(thread.into()),
            names,
        };
        Ok(self.client.add_labels(request).await?.into_inner().names)
    }

    /// Tell the daemon an agent started `thread` in `dir`, before any message of it is captured.
    /// A `dir` that isn't UTF-8 can't be named to it.
    pub async fn thread_started(
        &mut self,
        thread: HarnessSession,
        dir: &Path,
    ) -> Result<(), tonic::Status> {
        let Some(dir) = dir.to_str() else {
            return Err(tonic::Status::invalid_argument(format!("{} is not UTF-8", dir.display())));
        };
        let request = ThreadStartedRequest {
            thread: Some(thread.into()),
            dir: dir.to_owned(),
        };
        built_with_octavo(self.client.thread_started(request).await)?;
        Ok(())
    }

    /// Who the thread an agent started lately as `id` will be shared with once its first message
    /// is captured, or [`None`] when no agent started one by that id, or the daemon was built
    /// without Octavo. The daemon waits a moment for a start it hasn't heard of yet.
    pub async fn new_thread_shares(
        &mut self,
        id: &str,
    ) -> Result<Option<ThreadShares>, tonic::Status> {
        let request = NewThreadRequest {
            session_id: id.to_owned(),
        };
        match built_with_octavo(self.client.get_new_thread_shares(request).await) {
            Err(status) if status.code() == Code::NotFound => Ok(None),
            reply => reply,
        }
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
