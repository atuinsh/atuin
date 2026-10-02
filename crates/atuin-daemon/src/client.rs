use std::num::NonZeroU32;
#[cfg(unix)]
use std::path::PathBuf;

use atuin_client::ai_session::{HarnessKind, HarnessSession, SearchTerms, SessionFilter};
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
use time::OffsetDateTime;
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tonic::Code;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{Level, instrument, span};

use crate::grpc::ai::agent::pb::Session as AiSession;
use crate::grpc::ai::session::pb::ai_session_client::AiSessionClient as AiSessionServiceClient;
use crate::grpc::ai::session::pb::{
    GetSessionEvent, GetSessionRequest, GetTranscriptChunk, GetTranscriptRequest,
    ImportSessionsEvent, ImportSessionsRequest, ListSessionsRequest, RebuildSessionsRequest,
    SearchSessionsMatch, SearchSessionsRequest, TailSessionsEvent, TailSessionsRequest,
};
use crate::grpc::history::pb::history_client::HistoryClient as HistoryServiceClient;
use crate::grpc::history::pb::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, CommandCapture, CommandCaptureMeta,
    CompactStoreReply, CompactStoreRequest, DeleteHistoryReply, DeleteHistoryRequest,
    EndHistoryReply, EndHistoryRequest, GetCommandOutputRequest, GetCommandOutputResponse,
    ImportHistoryReply, RebuildHistoryReply, RebuildHistoryRequest, RegisterCommandOutputRequest,
    ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply, StatusRequest,
    TailHistoryReply, TailHistoryRequest, import_requests,
};
use crate::output_capture::OutputMatch;
use crate::search::search_client::SearchClient as SearchServiceClient;
use crate::search::{
    FilterMode as RpcFilterMode, PrepareIndexRequest, SearchCommandOutputRequest,
    SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
};

/// The path to the daemon's socket.
///
/// If the daemon is running and has recorded its socket path in the pidfile, this function returns
/// that. Otherwise, this function returns [`settings.daemon.existing_socket_path()`][0].
///
/// As an exception, if [`systemd_socket`][1] is true, the pidfile isn't consulted, as the socket
/// path comes from systemd directly through a file descriptor.
///
/// [0]: atuin_client::settings::Daemon::existing_socket_path
/// [1]: atuin_client::settings::Daemon::systemd_socket
#[cfg(unix)]
#[must_use]
pub fn socket_path(settings: &atuin_client::settings::Settings) -> PathBuf {
    (!settings.daemon.systemd_socket)
        .then(|| {
            crate::pidfile::PidfileInfo::read(settings.daemon.pidfile_path.as_ref())
                .and_then(|info| info.socket_path)
        })
        .flatten()
        .unwrap_or_else(|| settings.daemon.existing_socket_path().into_owned())
}

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
        Self::new(socket_path(settings)).await
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

    /// Import finished history (e.g. from a shell's history file), streamed so no message exceeds
    /// the daemon's size limit however long the commands are; see [`import_requests`].
    pub async fn import_history(&mut self, histories: Vec<History>) -> Result<ImportHistoryReply> {
        let requests = import_requests(histories);
        Ok(self.client.import_history(futures::stream::iter(requests)).await?.into_inner())
    }

    pub async fn compact_store(&mut self) -> Result<CompactStoreReply> {
        Ok(self.client.compact_store(CompactStoreRequest {}).await?.into_inner())
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
        Self::new(socket_path(settings)).await
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

/// First and longest waits between [`AiClient::wait_for_sessions`] probes.
const REBUILD_POLL_START: std::time::Duration = std::time::Duration::from_millis(250);
const REBUILD_POLL_MAX: std::time::Duration = std::time::Duration::from_secs(5);

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
        Self::new(socket_path(settings)).await
    }

    #[cfg(not(unix))]
    pub async fn from_settings(settings: &Settings) -> Result<Self> {
        Self::new(settings.daemon.tcp_port).await
    }

    /// Wait while the daemon is still rebuilding AI sessions after starting: until then every
    /// session read but a tail is refused rather than answered partially. Calls `on_wait` each
    /// time it finds the daemon rebuilding, with how far it has got (records replayed, and
    /// roughly how many there are to replay) when the daemon says, and returns at once when it
    /// is not. After this the daemon serves reads until a store command has it rebuild again
    /// (`atuin store rebuild ai-session`, a purge or a forced pull), which is rare.
    pub async fn wait_for_sessions(
        &mut self,
        mut on_wait: impl FnMut(Option<(u64, u64)>),
    ) -> Result<()> {
        let mut delay = REBUILD_POLL_START;
        while let Some(progress) = self.rebuild_status().await? {
            on_wait(progress);
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(REBUILD_POLL_MAX);
        }
        Ok(())
    }

    /// Whether the daemon is rebuilding AI sessions right now (see [`Self::wait_for_sessions`]),
    /// without waiting for it to finish: `Some` while it is, with how far it has got (records
    /// replayed, and roughly how many there are to replay) when the daemon says; `None` when it
    /// serves reads.
    pub async fn rebuild_status(&mut self) -> Result<Option<Option<(u64, u64)>>> {
        // A listing filtered to the future matches nothing, so the probe costs little beyond the
        // rebuild check every read makes first.
        let future = OffsetDateTime::now_utc() + time::Duration::days(365);
        let probe = list_sessions_request(&SessionFilter {
            updated_since: Some(future),
            ..SessionFilter::default()
        });
        match self.client.list_sessions(probe).await {
            Err(status) if crate::grpc::ai::session::is_rebuilding(&status) => {
                Ok(Some(crate::grpc::ai::session::rebuild_progress(&status)))
            }
            Err(status) => Err(status.into()),
            Ok(_) => Ok(None),
        }
    }

    /// Stream captured session summaries passing `filter`, newest first. The daemon sends one
    /// session per message (so a long list never trips the gRPC message-size limit); callers that
    /// want the whole set collect it with `try_collect`, and ones that only want the newest can
    /// take the first item without draining the rest.
    pub async fn list_sessions(
        &mut self,
        filter: &SessionFilter,
    ) -> Result<tonic::Streaming<AiSession>> {
        let request = list_sessions_request(filter);
        Ok(self.client.list_sessions(request).await?.into_inner())
    }

    /// Stream one session: the first event carries the [`AiSession`], each event after it a message.
    pub async fn get_session(
        &mut self,
        session: HarnessSession,
    ) -> Result<tonic::Streaming<GetSessionEvent>> {
        let request = GetSessionRequest {
            session: Some(session.into()),
        };
        Ok(self.client.get_session(request).await?.into_inner())
    }

    /// Stream a rendered plain-text transcript in chunks; concatenate them in arrival order.
    pub async fn get_transcript(
        &mut self,
        session: HarnessSession,
    ) -> Result<tonic::Streaming<GetTranscriptChunk>> {
        let request = GetTranscriptRequest {
            session: Some(session.into()),
        };
        Ok(self.client.get_transcript(request).await?.into_inner())
    }

    /// Follow sessions and messages as they are recorded. `harness` filters to one harness when set.
    pub async fn tail_sessions(
        &mut self,
        harness: Option<HarnessKind>,
    ) -> Result<tonic::Streaming<TailSessionsEvent>> {
        let request = TailSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.tail_sessions(request).await?.into_inner())
    }

    /// Stream the sessions matching `query` (its terms matching as `terms` says) and passing
    /// `filter`, most relevant first, at most `limit` (0 is unbounded). An empty query streams
    /// them newest first.
    ///
    /// The daemon matches every term as a whole word, or with [`SearchTerms::Any`] any term as a
    /// prefix; [`SearchTerms::Typed`] (search as you type) is searched as [`SearchTerms::All`].
    pub async fn search_sessions(
        &mut self,
        query: &str,
        terms: SearchTerms,
        filter: &SessionFilter,
        limit: u32,
    ) -> Result<tonic::Streaming<SearchSessionsMatch>> {
        let request = search_sessions_request(query, terms, filter, limit);
        Ok(self.client.search_sessions(request).await?.into_inner())
    }

    /// Have the daemon rebuild its sessions from the record store, after records were deleted
    /// under them. Returns once what they projected is gone; the replay goes on in the
    /// background, and [`Self::wait_for_sessions`] waits it out.
    pub async fn rebuild_sessions(&mut self) -> Result<()> {
        self.client.rebuild_sessions(RebuildSessionsRequest {}).await?;
        Ok(())
    }

    pub async fn import_sessions(
        &mut self,
        harness: Option<HarnessKind>,
    ) -> Result<tonic::Streaming<ImportSessionsEvent>> {
        let request = ImportSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.import_sessions(request).await?.into_inner())
    }
}

/// A listing of the sessions passing `filter`. The harness and `updated_since` go in the bare
/// fields as well as the filter, for a daemon from before `filter` (still running after an
/// upgrade), which ignores it; a newer one reads the filter's first, so the two never disagree.
fn list_sessions_request(filter: &SessionFilter) -> ListSessionsRequest {
    ListSessionsRequest {
        harness: filter.harness.map(|h| h as i32),
        updated_since: filter.updated_since.map(|ts| prost_types::Timestamp {
            seconds: ts.unix_timestamp(),
            nanos: ts.nanosecond().cast_signed(),
        }),
        filter: Some(filter.into()),
    }
}

/// A search for `query` over the sessions passing `filter`, with the harness and workspace in the
/// bare fields too (see [`list_sessions_request`]).
fn search_sessions_request(
    query: &str,
    terms: SearchTerms,
    filter: &SessionFilter,
    limit: u32,
) -> SearchSessionsRequest {
    SearchSessionsRequest {
        query: query.to_owned(),
        limit,
        harness: filter.harness.map(|h| h as i32),
        cwd: filter.workspace.as_ref().map(|w| w.to_string_lossy().into_owned()),
        any_term: terms == SearchTerms::Any,
        filter: Some(filter.into()),
    }
}

#[cfg(test)]
mod request_tests {
    use atuin_client::ai_session::{HarnessKind, SearchTerms, SessionFilter};
    use rstest::rstest;

    use super::{list_sessions_request, search_sessions_request};

    /// An older daemon reads only the legacy `harness` field, so it must carry the filter's.
    #[rstest]
    #[case::none(None)]
    #[case::codex(Some(HarnessKind::Codex))]
    fn requests_carry_the_harness_in_the_legacy_field(#[case] harness: Option<HarnessKind>) {
        let filter = SessionFilter {
            harness,
            branch: Some("main".to_owned()),
            ..SessionFilter::default()
        };
        let expected = harness.map(|h| h as i32);

        let list = list_sessions_request(&filter);
        assert_eq!(list.harness, expected);
        assert_eq!(list.filter.as_ref().and_then(|f| f.harness), expected);
        assert_eq!(list.filter.and_then(|f| f.branch).as_deref(), Some("main"));

        let search = search_sessions_request("words", SearchTerms::All, &filter, 7);
        assert_eq!(search.harness, expected);
        assert_eq!(search.filter.as_ref().and_then(|f| f.harness), expected);
        assert_eq!((search.query.as_str(), search.limit), ("words", 7));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    use rstest::rstest;

    use super::*;
    use crate::pidfile::PidfileGuard;

    /// Settings for a client whose daemon socket is configured at `socket_path`.
    fn settings(pidfile: &Path, socket_path: &Path, systemd_socket: bool) -> Settings {
        let mut settings = Settings::default();
        settings.daemon.pidfile_path = pidfile.to_str().unwrap().to_string();
        settings.daemon.socket_path = Some(socket_path.to_owned());
        settings.daemon.systemd_socket = systemd_socket;
        settings
    }

    /// A daemon configured for `old.sock` has exited, leaving its pidfile behind, and the client is
    /// now configured for `new.sock`.
    #[rstest]
    #[case::pidfile_path_is_used(false, false, "old.sock")]
    #[case::systemd_client_ignores_pidfile(false, true, "new.sock")]
    #[case::systemd_daemon_records_no_path(true, false, "new.sock")]
    #[case::both_systemd(true, true, "new.sock")]
    fn test_socket_path_with_stale_pidfile(
        #[case] daemon_systemd_socket: bool,
        #[case] client_systemd_socket: bool,
        #[case] expected: &str,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("atuin-daemon.pid");

        let old = settings(&pidfile, &dir.path().join("old.sock"), daemon_systemd_socket);
        drop(PidfileGuard::acquire(&old.daemon).unwrap());

        let new = settings(&pidfile, &dir.path().join("new.sock"), client_systemd_socket);
        assert_eq!(socket_path(&new), dir.path().join(expected));
    }

    #[rstest]
    fn test_socket_path_without_pidfile(#[values(false, true)] systemd_socket: bool) {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(
            &dir.path().join("atuin-daemon.pid"),
            &dir.path().join("a.sock"),
            systemd_socket,
        );
        assert_eq!(socket_path(&settings), dir.path().join("a.sock"));
    }
}

#[cfg(all(test, unix))]
mod rebuild_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rstest::rstest;
    use tokio::net::UnixListener;
    use tokio::sync::watch;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    use super::AiClient;
    use crate::grpc::AiSessionService;
    use crate::grpc::ai::session::pb::ai_session_server::AiSessionServer;
    use crate::session_capture::{AiHarnessSessionCapture, StoreState};

    /// A client of an AI-session service over a real socket, in the given recovery state.
    async fn serve(state: StoreState) -> (AiClient, watch::Sender<StoreState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let (capture, state) = AiHarnessSessionCapture::with_state(state).await;
        let service = AiSessionServer::new(AiSessionService::new(Arc::new(capture)));
        let incoming = UnixListenerStream::new(UnixListener::bind(&path).unwrap());
        tokio::spawn(Server::builder().add_service(service).serve_with_incoming(incoming));
        (AiClient::new(path).await.unwrap(), state, dir)
    }

    #[rstest]
    #[tokio::test]
    async fn waits_out_a_rebuild_over_the_wire() {
        let (mut client, state, _dir) = serve(StoreState::Recovering).await;
        let notices = Arc::new(AtomicUsize::new(0));
        let counter = notices.clone();
        let wait = tokio::spawn(async move {
            client
                .wait_for_sessions(|progress| {
                    // The test's store replays nothing: the daemon reports that it has none to.
                    assert_eq!(progress, Some((0, 0)));
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!wait.is_finished(), "returned while the daemon was rebuilding");

        state.send_replace(StoreState::Ready);
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .expect("returns once the rebuild ends")
            .unwrap()
            .unwrap();
        // Probed at once, then after 250ms: each probe that finds it rebuilding reports.
        assert!(notices.load(Ordering::SeqCst) >= 2, "each wait reports its progress");
    }

    #[rstest]
    #[case::ready(StoreState::Ready)]
    #[case::failed(StoreState::Unavailable)]
    #[tokio::test]
    async fn returns_at_once_when_not_rebuilding(#[case] state: StoreState) {
        let (mut client, _state, _dir) = serve(state).await;

        let mut announced = false;
        client.wait_for_sessions(|_| announced = true).await.unwrap();

        assert!(!announced);
    }

    /// The status a probe reads without waiting: rebuilding (with its progress) until the
    /// rebuild ends, then not.
    #[rstest]
    #[tokio::test]
    async fn reads_the_rebuild_status_without_waiting() {
        let (mut client, state, _dir) = serve(StoreState::Recovering).await;
        // The test's store replays nothing: the daemon reports that it has none to.
        assert_eq!(client.rebuild_status().await.unwrap(), Some(Some((0, 0))));
        state.send_replace(StoreState::Ready);
        assert_eq!(client.rebuild_status().await.unwrap(), None);
    }
}
