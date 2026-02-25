use eyre::{Context, Result};
#[cfg(windows)]
use tokio::net::TcpStream;
use tonic::Code;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use hyper_util::rt::TokioIo;

#[cfg(unix)]
use tokio::net::UnixStream;

use atuin_client::history::History;

use crate::history::{
    EndHistoryReply, EndHistoryRequest, ShutdownRequest, StartHistoryReply, StartHistoryRequest,
    StatusReply, StatusRequest, history_client::HistoryClient as HistoryServiceClient,
};

pub struct HistoryClient {
    client: HistoryServiceClient<Channel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonClientErrorKind {
    Connect,
    Unavailable,
    Unimplemented,
    Other,
}

#[must_use]
pub fn classify_error(error: &eyre::Report) -> DaemonClientErrorKind {
    for cause in error.chain() {
        if cause.downcast_ref::<tonic::transport::Error>().is_some() {
            return DaemonClientErrorKind::Connect;
        }

        if let Some(status) = cause.downcast_ref::<tonic::Status>() {
            return match status.code() {
                Code::Unavailable => DaemonClientErrorKind::Unavailable,
                Code::Unimplemented => DaemonClientErrorKind::Unimplemented,
                _ => DaemonClientErrorKind::Other,
            };
        }
    }

    DaemonClientErrorKind::Other
}

// Wrap the grpc client
impl HistoryClient {
    #[cfg(unix)]
    pub async fn new(path: String) -> Result<Self> {
        let log_path = path.clone();
        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = path.clone();

                async move {
                    Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path.clone()).await?))
                }
            }))
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to connect to local atuin daemon at {}. Is it running?",
                    &log_path
                )
            })?;

        let client = HistoryServiceClient::new(channel);

        Ok(HistoryClient { client })
    }

    #[cfg(not(unix))]
    pub async fn new(port: u64) -> Result<Self> {
        let channel = Endpoint::try_from("http://atuin_local_daemon:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let url = format!("127.0.0.1:{port}");

                async move {
                    Ok::<_, std::io::Error>(TokioIo::new(TcpStream::connect(url.clone()).await?))
                }
            }))
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to connect to local atuin daemon at 127.0.0.1:{port}. Is it running?"
                )
            })?;

        let client = HistoryServiceClient::new(channel);

        Ok(HistoryClient { client })
    }

    pub async fn start_history(&mut self, h: History) -> Result<StartHistoryReply> {
        let req = StartHistoryRequest {
            command: h.command,
            cwd: h.cwd,
            hostname: h.hostname,
            session: h.session,
            timestamp: h.timestamp.unix_timestamp_nanos() as u64,
            author: h.author,
            intent: h.intent.unwrap_or_default(),
        };

        Ok(self.client.start_history(req).await?.into_inner())
    }

    pub async fn end_history(
        &mut self,
        id: String,
        duration: u64,
        exit: i64,
    ) -> Result<EndHistoryReply> {
        let req = EndHistoryRequest { id, duration, exit };

        Ok(self.client.end_history(req).await?.into_inner())
    }

    pub async fn status(&mut self) -> Result<StatusReply> {
        Ok(self.client.status(StatusRequest {}).await?.into_inner())
    }

    pub async fn shutdown(&mut self) -> Result<bool> {
        let resp = self.client.shutdown(ShutdownRequest {}).await?.into_inner();
        Ok(resp.accepted)
    }
}
