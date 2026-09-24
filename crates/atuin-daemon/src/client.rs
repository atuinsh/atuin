use std::num::NonZeroU32;
#[cfg(unix)]
use std::path::PathBuf;

use atuin_client::database::Context;
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::{FilterMode, Settings};
use atuin_common::filter::{self, OrFilter};
use atuin_common::range::PyStyleIdxRange;
use easy_cast::Conv;
use eyre::{Context as EyreContext, Result};
use futures::{Stream, StreamExt};
use hyper_util::rt::TokioIo;
use itertools::Itertools;
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tonic::Code;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{Level, instrument, span};

use crate::grpc::ai::agent::pb::{
    HarnessKind as AiHarnessKind, HarnessSession as AiHarnessSession, Session as AiSession,
};
use crate::grpc::ai::session::pb::ai_session_client::AiSessionClient as AiSessionServiceClient;
use crate::grpc::ai::session::pb::{
    GetSessionEvent, GetSessionRequest, GetTranscriptChunk, GetTranscriptRequest,
    ImportSessionsEvent, ImportSessionsRequest, ListSessionsRequest, SearchSessionsMatch,
    SearchSessionsRequest, TailSessionsEvent, TailSessionsRequest,
};
use crate::grpc::history::pb::history_client::HistoryClient as HistoryServiceClient;
use crate::grpc::history::pb::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, CommandCapture, CommandCaptureMeta,
    DeleteHistoryReply, DeleteHistoryRequest, EndHistoryReply, EndHistoryRequest,
    GetCommandOutputRequest, GetCommandOutputResponse, RebuildHistoryReply, RebuildHistoryRequest,
    RegisterCommandOutputRequest, ShutdownRequest, StartHistoryReply, StartHistoryRequest,
    StatusReply, StatusRequest, TailHistoryReply, TailHistoryRequest,
};
use crate::output_capture::OutputMatch;
use crate::search::search_client::SearchClient as SearchServiceClient;
use crate::search::{
    FilterMode as RpcFilterMode, PrepareIndexRequest, SearchCommandOutputRequest,
    SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
};

pub struct HistoryClient {
    client: HistoryServiceClient<Channel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonClientErrorKind {
    Connect,
    Unavailable,
    Unimplemented,
    OtherGrpc,
    NonGrpc,
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
                _ => DaemonClientErrorKind::OtherGrpc,
            };
        }
    }

    DaemonClientErrorKind::NonGrpc
}

// Wrap the grpc client
impl HistoryClient {
    #[cfg(unix)]
    pub async fn new(path: PathBuf) -> Result<Self> {
        use eyre::Context;

        let log_path = path.clone();
        let channel =
            Endpoint::try_from("http://atuin_local_daemon:0")?
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();

                    async move {
                        Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?))
                    }
                }))
                .await
                .wrap_err_with(|| {
                    format!(
                        "failed to connect to local atuin daemon at {}. Is it running?",
                        log_path.display()
                    )
                })?;

        let client = HistoryServiceClient::new(channel);

        Ok(Self { client })
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

    #[cfg(unix)]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.existing_socket_path().into_owned()).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.tcp_port).await
    }

    pub async fn start_history(&mut self, h: History) -> Result<StartHistoryReply> {
        let req = StartHistoryRequest {
            command: h.command,
            cwd: h.cwd,
            hostname: h.cmd_origin.into_string(),
            session: h.session,
            timestamp: i64::conv(h.timestamp.unix_timestamp_nanos()),
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
    ) -> Result<EndHistoryReply> {
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

    pub async fn cancel_history(&mut self, id: HistoryId) -> Result<CancelHistoryReply> {
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
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<DeleteHistoryReply> {
        // TODO(markovejnovic): A more flexible implementation would be to iterate into chunks that
        //                      are as large as possible. If we know the size of a struct (which we
        //                      can, in theory), then we can simply chunk by that.
        //                      If we _don't_ know the size of the struct, then we can create a
        //                      struct, measure its size, repeat until we reach our threshold.
        const DELETE_CHUNK_SIZE: usize = 50_000;

        let chunks: Vec<DeleteHistoryRequest> = ids
            .into_iter()
            .chunks(DELETE_CHUNK_SIZE)
            .into_iter()
            .map(|chunk| DeleteHistoryRequest {
                ids: chunk.map(Into::into).collect(),
            })
            .collect();

        Ok(self.client.delete_history(futures::stream::iter(chunks)).await?.into_inner())
    }

    pub async fn rebuild_history(&mut self) -> Result<RebuildHistoryReply> {
        Ok(self.client.rebuild_history(RebuildHistoryRequest {}).await?.into_inner())
    }

    pub async fn status(&mut self) -> Result<StatusReply> {
        Ok(self.client.status(StatusRequest {}).await?.into_inner())
    }

    pub async fn tail_history(&mut self) -> Result<tonic::Streaming<TailHistoryReply>> {
        Ok(self.client.tail_history(TailHistoryRequest {}).await?.into_inner())
    }

    pub async fn shutdown(&mut self) -> Result<bool> {
        let resp = self.client.shutdown(ShutdownRequest {}).await?.into_inner();
        Ok(resp.accepted)
    }

    pub async fn register_command_output(
        &mut self,
        id: HistoryId,
        output_start: impl Into<String>,
        output_end: Option<String>,
        output_observed_bytes: u64,
        terminal_width: u16,
        terminal_height: u16,
    ) -> Result<()> {
        let capture = CommandCapture {
            output_start: output_start.into(),
            output_end,
            meta: Some(CommandCaptureMeta {
                output_observed_bytes,
                terminal_width: terminal_width.into(),
                terminal_height: terminal_height.into(),
            }),
        };
        self.client
            .register_command_output(RegisterCommandOutputRequest {
                history_id: Some(id.into()),
                capture: Some(capture),
            })
            .await?;
        Ok(())
    }

    /// Fetch a command's captured output for the requested line ranges. Returns [`None`] when the
    /// daemon has no output stored for `id` (the daemon signals this with a `NOT_FOUND` status).
    pub async fn get_command_output(
        &mut self,
        id: HistoryId,
        ranges: Vec<PyStyleIdxRange>,
    ) -> Result<Option<GetCommandOutputResponse>> {
        let request = GetCommandOutputRequest {
            id: Some(id.into()),
            line_ranges: ranges,
        };
        match self.client.get_command_output(request).await {
            Ok(response) => Ok(Some(response.into_inner())),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(status.into()),
        }
    }
}

#[derive(Clone)]
pub struct SearchParams {
    pub query: String,
    pub query_id: u64,
    pub filter_mode: FilterMode,
    pub context: Option<Context>,
    pub shells: OrFilter<Vec<String>>,
}

impl From<SearchParams> for SearchRequest {
    fn from(params: SearchParams) -> Self {
        Self {
            query: params.query,
            query_id: params.query_id,
            filter_mode: RpcFilterMode::from(params.filter_mode).into(),
            context: params.context.map(RpcSearchContext::from),
            // An empty list in `SearchRequest::shells` means "all".
            shells: match params.shells.into_list() {
                filter::Items::All => vec![],
                filter::Items::Some(vec) => vec,
            },
        }
    }
}

pub struct SearchClient {
    client: SearchServiceClient<Channel>,
}

impl SearchClient {
    #[cfg(unix)]
    pub async fn new(path: PathBuf) -> Result<Self> {
        let log_path = path.clone();
        let channel =
            Endpoint::try_from("http://atuin_local_daemon:0")?
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();

                    async move {
                        Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?))
                    }
                }))
                .await
                .wrap_err_with(|| {
                    format!(
                        "failed to connect to local atuin daemon at {}. Is it running?",
                        log_path.display()
                    )
                })?;

        let client = SearchServiceClient::new(channel);

        Ok(Self { client })
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

    #[cfg(unix)]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.existing_socket_path().into_owned()).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.tcp_port).await
    }

    #[instrument(
        skip_all,
        level = Level::TRACE,
        name = "daemon_client_search",
        fields(query = %params.query, query_id = params.query_id),
    )]
    pub async fn search(
        &mut self,
        params: SearchParams,
    ) -> Result<tonic::Streaming<SearchResponse>> {
        let request = SearchRequest::from(params);
        let request_stream = tokio_stream::once(request);
        let response = span!(Level::TRACE, "daemon_client_search.request")
            .in_scope(async || self.client.search(request_stream).await)
            .await?;

        Ok(response.into_inner())
    }

    #[instrument(
        skip_all,
        level = Level::TRACE,
        name = "search_command_output",
    )]
    /// Relevance-ranked hits, each reduced to the lines within `context` of a match, or whole
    /// when `context` is `None`.
    pub async fn search_command_output(
        &mut self,
        query: String,
        limit: Option<NonZeroU32>,
        context: Option<u32>,
    ) -> Result<impl Stream<Item = Result<OutputMatch>> + Send + use<>> {
        let request = SearchCommandOutputRequest {
            query,
            limit: limit.map_or(0, NonZeroU32::get),
            context,
        };
        let stream = self.client.search_command_output(request).await?.into_inner();
        Ok(stream.map(|item| -> Result<OutputMatch> { Ok(OutputMatch::try_from(item?)?) }))
    }

    /// Tell the daemon to build the search index for the given list of shells.
    pub async fn prepare_index(&mut self, shells: OrFilter<Vec<String>>) -> Result<()> {
        let request = PrepareIndexRequest {
            // Same as `SearchRequest::shells` -- empty list means "all".
            shells: match shells.into_list() {
                filter::Items::All => vec![],
                filter::Items::Some(vec) => vec,
            },
        };
        self.client.prepare_index(request).await?;
        Ok(())
    }
}

impl From<FilterMode> for RpcFilterMode {
    fn from(filter_mode: FilterMode) -> Self {
        match filter_mode {
            FilterMode::Global => Self::Global,
            FilterMode::Host => Self::Host,
            FilterMode::Session => Self::Session,
            FilterMode::Directory => Self::Directory,
            FilterMode::Workspace => Self::Workspace,
            FilterMode::SessionPreload => Self::SessionPreload,
        }
    }
}

impl From<Context> for RpcSearchContext {
    fn from(context: Context) -> Self {
        Self {
            session_id: context.session,
            cwd: context.cwd,
            hostname: context.cmd_origin.into_string(),
            host_id: context.host_id,
            git_root: context.git_root.map(|path| path.to_string_lossy().to_string()),
        }
    }
}

/// Client for the daemon's `ai.session.AiSession` service. Wraps the generated tonic stub the same
/// way [`HistoryClient`] and [`SearchClient`] do, returning the raw protobuf messages so callers can
/// render them however they like.
pub struct AiClient {
    client: AiSessionServiceClient<Channel>,
}

impl AiClient {
    #[cfg(unix)]
    pub async fn new(path: PathBuf) -> Result<Self> {
        let log_path = path.clone();
        let channel =
            Endpoint::try_from("http://atuin_local_daemon:0")?
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();

                    async move {
                        Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?))
                    }
                }))
                .await
                .wrap_err_with(|| {
                    format!(
                        "failed to connect to local atuin daemon at {}. Is it running?",
                        log_path.display()
                    )
                })?;

        Ok(Self {
            client: AiSessionServiceClient::new(channel),
        })
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

        Ok(Self {
            client: AiSessionServiceClient::new(channel),
        })
    }

    #[cfg(unix)]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.existing_socket_path().into_owned()).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.tcp_port).await
    }

    /// Stream captured session summaries, newest first. `harness` filters to a single harness when
    /// set. The daemon sends one session per message (so a long list never trips the gRPC
    /// message-size limit); callers that want the whole set collect it with `try_collect`, and ones
    /// that only want the newest can take the first item without draining the rest.
    pub async fn list_sessions(
        &mut self,
        harness: Option<AiHarnessKind>,
    ) -> Result<tonic::Streaming<AiSession>> {
        let request = ListSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.list_sessions(request).await?.into_inner())
    }

    /// Stream one session: the first event carries the [`AiSession`], each event after it a message.
    pub async fn get_session(
        &mut self,
        session: AiHarnessSession,
    ) -> Result<tonic::Streaming<GetSessionEvent>> {
        let request = GetSessionRequest {
            session: Some(session),
        };
        Ok(self.client.get_session(request).await?.into_inner())
    }

    /// Stream a rendered plain-text transcript in chunks; concatenate them in arrival order.
    pub async fn get_transcript(
        &mut self,
        session: AiHarnessSession,
    ) -> Result<tonic::Streaming<GetTranscriptChunk>> {
        let request = GetTranscriptRequest {
            session: Some(session),
        };
        Ok(self.client.get_transcript(request).await?.into_inner())
    }

    /// Follow sessions and messages as they are recorded. `harness` filters to one harness when set.
    pub async fn tail_sessions(
        &mut self,
        harness: Option<AiHarnessKind>,
    ) -> Result<tonic::Streaming<TailSessionsEvent>> {
        let request = TailSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.tail_sessions(request).await?.into_inner())
    }

    pub async fn search_sessions(
        &mut self,
        query: &str,
        harness: Option<AiHarnessKind>,
        limit: u32,
    ) -> Result<tonic::Streaming<SearchSessionsMatch>> {
        let request = SearchSessionsRequest {
            query: query.to_owned(),
            limit,
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.search_sessions(request).await?.into_inner())
    }

    pub async fn import_sessions(
        &mut self,
        harness: Option<AiHarnessKind>,
    ) -> Result<tonic::Streaming<ImportSessionsEvent>> {
        let request = ImportSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.import_sessions(request).await?.into_inner())
    }
}
