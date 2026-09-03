//! Client-side wrapper around the daemon's History gRPC service.
//!
//! This exposes [`HistoryClient`] and nothing more: a thin, ergonomic wrapper
//! over the generated [`HistoryServiceClient`] that connects to the local
//! daemon and wraps only the History RPCs.

#[cfg(unix)]
use std::path::PathBuf;

use atuin_client::history::{History, HistoryId};
use easy_cast::Conv;
use hyper_util::rt::TokioIo;
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::grpc::history::pb::history_client::HistoryClient as HistoryServiceClient;
use crate::grpc::history::pb::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, DeleteHistoryReply, DeleteHistoryRequest,
    EndHistoryReply, EndHistoryRequest, RebuildHistoryReply, RebuildHistoryRequest,
    ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply, StatusRequest,
    TailHistoryReply, TailHistoryRequest,
};

/// An error returned by [`HistoryClient`].
///
/// Each variant keeps its underlying error as the [`std::error::Error::source`], so callers that
/// still work in terms of a boxed/`eyre` error can downcast through the chain — for example to read
/// a [`tonic::Status`] code.
#[derive(Debug, thiserror::Error)]
pub enum HistoryClientError {
    /// Could not establish a connection to the local daemon.
    #[error("failed to connect to the local atuin daemon; is it running?")]
    Connect(#[from] tonic::transport::Error),

    /// The daemon returned a gRPC error status.
    #[error("history daemon rpc failed: {0}")]
    Rpc(#[from] tonic::Status),

    /// The measured command duration could not be encoded for the wire.
    #[error("invalid command duration: {0}")]
    Duration(#[from] prost_types::DurationError),
}

/// A wrapper around the generated History gRPC client.
pub struct HistoryClient {
    client: HistoryServiceClient<Channel>,
}

// Wrap the grpc client
impl HistoryClient {
    #[cfg(unix)]
    pub async fn new(path: PathBuf) -> Result<Self, HistoryClientError> {
        let channel =
            Endpoint::try_from("http://atuin_local_daemon:0")?
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();

                    async move {
                        Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?))
                    }
                }))
                .await?;

        let client = HistoryServiceClient::new(channel);

        Ok(Self { client })
    }

    #[cfg(not(unix))]
    pub async fn new(port: u64) -> Result<Self, HistoryClientError> {
        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let url = format!("127.0.0.1:{port}");

                async move {
                    Ok::<_, std::io::Error>(TokioIo::new(TcpStream::connect(url.clone()).await?))
                }
            }))
            .await?;

        let client = HistoryServiceClient::new(channel);

        Ok(HistoryClient { client })
    }

    pub async fn start_history(
        &mut self,
        h: History,
    ) -> Result<StartHistoryReply, HistoryClientError> {
        let req = StartHistoryRequest {
            command: h.command,
            cwd: h.cwd,
            hostname: h.cmd_origin.into_string(),
            session: h.session,
            timestamp: u64::conv(h.timestamp.unix_timestamp_nanos()),
            author: h.author,
            intent: h.intent.unwrap_or_default(),
            shell: h.shell.unwrap_or_default(),
            author_kind: AuthorKind::from(h.author_kind) as i32,
        };

        Ok(self.client.start_history(req).await?.into_inner())
    }

    pub async fn end_history(
        &mut self,
        id: HistoryId,
        duration: Option<std::time::Duration>,
        exit: i64,
    ) -> Result<EndHistoryReply, HistoryClientError> {
        let duration = duration.map(prost_types::Duration::try_from).transpose()?;
        Ok(self
            .client
            .end_history(EndHistoryRequest {
                id: Some(id.into()),
                duration,
                exit,
            })
            .await?
            .into_inner())
    }

    pub async fn cancel_history(
        &mut self,
        id: HistoryId,
    ) -> Result<CancelHistoryReply, HistoryClientError> {
        Ok(self
            .client
            .cancel_history(CancelHistoryRequest {
                id: Some(id.into()),
            })
            .await?
            .into_inner())
    }

    pub async fn delete_history(
        &mut self,
        ids: Vec<HistoryId>,
    ) -> Result<DeleteHistoryReply, HistoryClientError> {
        Ok(self
            .client
            .delete_history(DeleteHistoryRequest {
                ids: ids.into_iter().map(Into::into).collect(),
            })
            .await?
            .into_inner())
    }

    pub async fn rebuild_history(&mut self) -> Result<RebuildHistoryReply, HistoryClientError> {
        Ok(self.client.rebuild_history(RebuildHistoryRequest {}).await?.into_inner())
    }

    pub async fn status(&mut self) -> Result<StatusReply, HistoryClientError> {
        Ok(self.client.status(StatusRequest {}).await?.into_inner())
    }

    pub async fn tail_history(
        &mut self,
    ) -> Result<tonic::Streaming<TailHistoryReply>, HistoryClientError> {
        Ok(self.client.tail_history(TailHistoryRequest {}).await?.into_inner())
    }

    pub async fn shutdown(&mut self) -> Result<bool, HistoryClientError> {
        let resp = self.client.shutdown(ShutdownRequest {}).await?.into_inner();
        Ok(resp.accepted)
    }
}
