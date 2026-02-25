use atuin_client::database::Context;
use atuin_client::settings::FilterMode;
use eyre::{Context as EyreContext, Result};
#[cfg(windows)]
use tokio::net::TcpStream;
use tonic::Code;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use hyper_util::rt::TokioIo;

#[cfg(unix)]
use tokio::net::UnixStream;

use atuin_client::history::History;
use tracing::{Level, instrument, span};

use crate::history::{
    EndHistoryReply, EndHistoryRequest, ShutdownRequest, StartHistoryReply, StartHistoryRequest,
    StatusReply, StatusRequest, history_client::HistoryClient as HistoryServiceClient,
};
use crate::search::{
    FilterMode as RpcFilterMode, SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
    search_client::SearchClient as SearchServiceClient,
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
        use eyre::Context;

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

pub struct SearchClient {
    client: SearchServiceClient<Channel>,
}

impl SearchClient {
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

        let client = SearchServiceClient::new(channel);

        Ok(SearchClient { client })
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

        let client = SearchServiceClient::new(channel);

        Ok(SearchClient { client })
    }

    #[instrument(skip_all, level = Level::TRACE, name = "daemon_client_search", fields(query = %query, query_id = query_id))]
    pub async fn search(
        &mut self,
        query: String,
        query_id: u64,
        filter_mode: FilterMode,
        context: Option<Context>,
    ) -> Result<tonic::Streaming<SearchResponse>> {
        let request = SearchRequest {
            query,
            query_id,
            filter_mode: RpcFilterMode::from(filter_mode).into(),
            context: context.map(RpcSearchContext::from),
        };
        let request_stream = tokio_stream::once(request);
        let response = span!(Level::TRACE, "daemon_client_search.request")
            .in_scope(async || self.client.search(request_stream).await)
            .await?;

        Ok(response.into_inner())
    }
}

impl From<FilterMode> for RpcFilterMode {
    fn from(filter_mode: FilterMode) -> Self {
        match filter_mode {
            FilterMode::Global => RpcFilterMode::Global,
            FilterMode::Host => RpcFilterMode::Host,
            FilterMode::Session => RpcFilterMode::Session,
            FilterMode::Directory => RpcFilterMode::Directory,
            FilterMode::Workspace => RpcFilterMode::Workspace,
            FilterMode::SessionPreload => RpcFilterMode::SessionPreload,
        }
    }
}

impl From<Context> for RpcSearchContext {
    fn from(context: Context) -> Self {
        RpcSearchContext {
            session_id: context.session,
            cwd: context.cwd,
            hostname: context.hostname,
            host_id: context.host_id,
            git_root: context
                .git_root
                .map(|path| path.to_string_lossy().to_string()),
        }
    }
}
