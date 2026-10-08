#[cfg(unix)]
use std::borrow::Cow;
use std::num::NonZeroU32;
#[cfg(unix)]
use std::path::{Path, PathBuf};

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
    GetSessionEvent, GetSessionRequest, ImportSessionsEvent, ImportSessionsRequest,
    ListSessionsRequest, RebuildSessionsRequest, SearchSessionsMatch, SearchSessionsRequest,
    TailSessionsEvent, TailSessionsRequest,
};
use crate::grpc::history::pb::history_client::HistoryClient as HistoryServiceClient;
use crate::grpc::history::pb::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, CommandCapture, CommandCaptureMeta,
    CompactStoreReply, CompactStoreRequest, DeleteHistoryReply, DeleteHistoryRequest,
    EndHistoryReply, EndHistoryRequest, GetCommandOutputRequest, GetCommandOutputResponse,
    RebuildHistoryReply, RebuildHistoryRequest, RegisterCommandOutputRequest, ShutdownRequest,
    StartHistoryReply, StartHistoryRequest, StatusReply, StatusRequest, TailHistoryReply,
    TailHistoryRequest,
};
use crate::output_capture::OutputMatch;
use crate::search::search_client::SearchClient as SearchServiceClient;
use crate::search::{
    FilterMode as RpcFilterMode, PrepareIndexRequest, SearchCommandOutputRequest,
    SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
};

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub enum SocketPathError {
    /// The socket is in use by another program.
    ///
    /// This error can be returned when `ATUIN_HOME` is set. Atuin started storing the socket path
    /// in the pidfile before `ATUIN_HOME` was introduced, so if there's no socket path in the
    /// pidfile but the socket already exists, it's an indication that the socket is in use by
    /// another program (potentially another Atuin daemon running with a different value of
    /// `ATUIN_HOME`).
    #[error("{}", Self::display_in_use(.path, *.user_defined))]
    InUse {
        /// The path to the socket.
        path: PathBuf,
        /// Whether the path came from `daemon.socket_path` in config.toml.
        user_defined: bool,
    },
}

#[cfg(unix)]
impl SocketPathError {
    /// Helper for displaying [`Self::InUse`].
    fn display_in_use(path: &Path, user_defined: bool) -> impl std::fmt::Display + use<'_> {
        std::fmt::from_fn(move |f| {
            writeln!(f, "daemon socket '{}' is in use by another program", path.display())?;
            let config_path = atuin_common::dirs::config_path("config.toml");
            if user_defined {
                write!(
                    f,
                    "please ensure `daemon.socket_path` has a unique value in {}",
                    config_path.display(),
                )
            } else {
                write!(
                    f,
                    "please set an explicit `daemon.socket_path` in {}",
                    config_path.display()
                )
            }
        })
    }
}

/// The path to the daemon's socket.
///
/// If the daemon is running and has recorded its socket path in the pidfile, this function returns
/// that. Otherwise:
///
/// * If `ATUIN_HOME` is unset (the default), [`settings.daemon.existing_socket_path()`][0] is
///   returned, which will check for existing sockets in legacy locations.
///
/// * If `ATUIN_HOME` is set, [`settings.daemon.preferred_socket_path()`][1] is returned, which does
///   not check for legacy sockets, as Atuin stopped using them before `ATUIN_HOME` was introduced.
///   In addition, an error is returned if that socket already exists: Atuin started storing the
///   socket path in the pidfile before `ATUIN_HOME`, so if an existing socket isn't in our pidfile,
///   it almost certainly isn't ours. This behavior is intended to reduce the chance of the client
///   connecting to the wrong daemon.
///
/// As an exception, if [`systemd_socket`][2] is true, the pidfile isn't consulted, as the socket
/// path comes from systemd directly through a file descriptor.
///
/// [0]: atuin_client::settings::Daemon::existing_socket_path
/// [1]: atuin_client::settings::Daemon::preferred_socket_path
/// [2]: atuin_client::settings::Daemon::systemd_socket
#[cfg(unix)]
pub fn socket_path(settings: &Settings) -> Result<PathBuf, SocketPathError> {
    use std::io::ErrorKind;
    use std::os::unix::net::UnixStream;

    use crate::pidfile::{self, PidfileInfo};

    let existing_socket_path = || settings.daemon.existing_socket_path().into_owned();
    if settings.daemon.systemd_socket {
        return Ok(existing_socket_path());
    }

    let pidfile_path: &Path = settings.daemon.pidfile_path.as_ref();
    let socket_path = PidfileInfo::read(pidfile_path).and_then(|info| info.socket_path);

    if !atuin_common::dirs::atuin_home_is_set() {
        return Ok(socket_path.unwrap_or_else(existing_socket_path));
    }

    // If `ATUIN_HOME` is set, require the pidfile to be live to avoid connecting to the wrong
    // daemon. This should potentially be done even when `ATUIN_HOME` isn't set, but that would be a
    // larger and possibly riskier change. For now, scope the check to when `ATUIN_HOME` is set,
    // which is when we would most expect there to be multiple daemons running on the user's system.
    if let Some(path) = socket_path.filter(|_| pidfile::is_live(pidfile_path)) {
        return Ok(path);
    }

    // If there's no socket path in the pidfile (or the pidfile was missing/stale) and `ATUIN_HOME`
    // is set, don't look for or connect to existing sockets. Atuin switched to storing the socket
    // path in the pidfile before `ATUIN_HOME` was introduced, so an existing socket is almost
    // certainly that of another program, potentially another daemon running with a different value
    // of `ATUIN_HOME`, which is conceptually a separate profile that we should not connect to.
    let path = settings.daemon.preferred_socket_path().into_owned();

    // Avoid connecting to an existing socket except if we get `ConnectionRefused`, which indicates
    // it's no longer in use.
    if path.exists()
        && !UnixStream::connect(&path).is_err_and(|e| e.kind() == ErrorKind::ConnectionRefused)
    {
        return Err(SocketPathError::InUse {
            path,
            user_defined: settings.daemon.socket_path.is_some(),
        });
    }
    Ok(path)
}

/// The paths that should be checked when looking for existing daemon sockets.
///
/// For the same reasons explained in [`socket_path`], when `ATUIN_HOME` is set, this function will
/// only yield the [preferred socket path][0] instead of also including the legacy paths.
///
/// [0]: atuin_client::settings::Daemon::preferred_socket_path
#[cfg(unix)]
pub fn potential_socket_paths(
    settings: &Settings,
) -> impl Iterator<Item = Cow<'_, Path>> + use<'_> {
    if atuin_common::dirs::atuin_home_is_set() {
        itertools::Either::Left(std::iter::once(settings.daemon.preferred_socket_path()))
    } else {
        itertools::Either::Right(settings.daemon.potential_socket_paths())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FromSettingsError {
    #[cfg(unix)]
    #[error(transparent)]
    SocketPath(#[from] SocketPathError),
    #[error(transparent)]
    CreateClient(#[from] eyre::Report),
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

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        #[cfg(unix)]
        let address = socket_path(settings)?;
        #[cfg(not(unix))]
        let address = settings.daemon.tcp_port;
        Self::new(address).await.map_err(Into::into)
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

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        #[cfg(unix)]
        let address = socket_path(settings)?;
        #[cfg(not(unix))]
        let address = settings.daemon.tcp_port;
        Self::new(address).await.map_err(Into::into)
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

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        #[cfg(unix)]
        let address = socket_path(settings)?;
        #[cfg(not(unix))]
        let address = settings.daemon.tcp_port;
        Self::new(address).await.map_err(Into::into)
    }

    /// Wait while the daemon is still rebuilding AI sessions after starting: until then every
    /// session read but a tail is refused rather than answered partially. Calls `on_wait` each
    /// time it finds the daemon rebuilding, with how far it has got (records replayed, and
    /// roughly how many there are to replay), and returns at once when it is not. After this the
    /// daemon serves reads until a store command has it rebuild again (`atuin store rebuild
    /// ai-session`, a purge or a forced pull), which is rare.
    pub async fn wait_for_sessions(&mut self, mut on_wait: impl FnMut((u64, u64))) -> Result<()> {
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
    /// replayed, and roughly how many there are to replay); `None` when it serves reads.
    pub async fn rebuild_status(&mut self) -> Result<Option<(u64, u64)>> {
        // A listing filtered to the future matches nothing, so the probe costs little beyond the
        // rebuild check every read makes first.
        let future = OffsetDateTime::now_utc() + time::Duration::days(365);
        let filter = SessionFilter {
            updated_since: Some(future),
            ..SessionFilter::default()
        };
        let probe = ListSessionsRequest {
            filter: Some((&filter).into()),
        };
        match self.client.list_sessions(probe).await {
            Ok(_) => Ok(None),
            Err(status) => match crate::grpc::ai::session::rebuild_progress(&status) {
                Some(progress) => Ok(Some(progress)),
                None => Err(status.into()),
            },
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
        let request = ListSessionsRequest {
            filter: Some(filter.into()),
        };
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
        let request = SearchSessionsRequest {
            query: query.to_owned(),
            limit,
            any_term: terms == SearchTerms::Any,
            filter: Some(filter.into()),
        };
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

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    use atuin_common::env::MockEnv;
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
        assert_eq!(socket_path(&new).unwrap(), dir.path().join(expected));
    }

    #[rstest]
    fn test_socket_path_without_pidfile(#[values(false, true)] systemd_socket: bool) {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(
            &dir.path().join("atuin-daemon.pid"),
            &dir.path().join("a.sock"),
            systemd_socket,
        );
        assert_eq!(socket_path(&settings).unwrap(), dir.path().join("a.sock"));
    }

    /// A mock environment with `HOME` and `TMPDIR` in `dir`, and `ATUIN_HOME` too if
    /// `atuin_home_set`, so that neither the settings nor the default socket path involve the
    /// real home or `/tmp`.
    fn env(dir: &Path, atuin_home_set: bool) -> MockEnv {
        let env = MockEnv::install();
        env.set("HOME", dir.join("home"));
        env.set("TMPDIR", dir.join("tmp"));
        if atuin_home_set {
            env.set("ATUIN_HOME", dir.join("profile"));
        }
        env
    }

    /// Listen on a Unix socket at `path`, as a running daemon does, until the listener is
    /// dropped. Dropping it leaves the socket file behind with nothing listening, as a daemon that
    /// was killed does.
    fn listen(path: &Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::net::UnixListener::bind(path).unwrap()
    }

    /// The default socket path for `dir` as [`env`] sets it up, or the configured one.
    fn preferred_path(dir: &Path, configured: bool) -> PathBuf {
        if configured {
            dir.join("configured.sock")
        } else {
            dir.join("tmp")
                .join(format!("atuin-{}", atuin_common::os::unix::uid()))
                .join("atuin.sock")
        }
    }

    /// Settings with the socket at [`preferred_path`], and a pidfile in `dir`.
    fn settings_in(dir: &Path, configured: bool) -> Settings {
        let mut settings =
            settings(&dir.join("atuin-daemon.pid"), &dir.join("configured.sock"), false);
        if !configured {
            settings.daemon.socket_path = None;
        }
        assert_eq!(settings.daemon.preferred_socket_path(), preferred_path(dir, configured));
        settings
    }

    /// With `ATUIN_HOME` set, a live socket that the pidfile doesn't name belongs to something
    /// else, such as a daemon for another `ATUIN_HOME`, so the client refuses it rather than
    /// connect. Without `ATUIN_HOME`, an existing socket is connected to as before.
    #[rstest]
    fn an_unrecorded_live_socket_is_refused_only_with_atuin_home(
        #[values(false, true)] atuin_home_set: bool,
        #[values(false, true)] configured: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), atuin_home_set);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        // Nothing's there yet, so it's free for this profile's daemon. (Without `ATUIN_HOME`, the
        // client would also fall back to sockets elsewhere, like the real `/tmp/atuin-$UID`.)
        if atuin_home_set {
            assert_eq!(socket_path(&settings).unwrap(), path);
        }

        let _listener = listen(&path);
        let result = socket_path(&settings);
        if !atuin_home_set {
            assert_eq!(result.unwrap(), path);
            return;
        }
        let err = result.unwrap_err();
        let message = err.to_string();
        let SocketPathError::InUse {
            path: in_use,
            user_defined,
        } = err;
        assert_eq!(in_use, path);
        assert_eq!(user_defined, configured);
        assert!(message.contains(&path.display().to_string()), "{message}");
        let hint = if configured {
            "has a unique value"
        } else {
            "set an explicit"
        };
        assert!(message.contains(hint), "{message}");
    }

    /// With `ATUIN_HOME` set, a socket left behind with nothing listening, as by a daemon that was
    /// killed, is free: the client uses it, and autostart replaces it.
    #[rstest]
    fn with_atuin_home_a_stale_socket_is_free(#[values(false, true)] configured: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        drop(listen(&path));
        assert!(path.exists(), "the stale socket should be left behind");
        assert_eq!(socket_path(&settings).unwrap(), path);
    }

    /// With `ATUIN_HOME` set, the socket the pidfile names is used while its daemon is running,
    /// whatever else exists.
    #[rstest]
    fn with_atuin_home_the_recorded_socket_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        let _guard = PidfileGuard::acquire(&daemon.daemon).unwrap();

        let client = settings(&pidfile, &dir.path().join("other.sock"), false);
        let _listener = listen(&dir.path().join("other.sock"));
        assert_eq!(socket_path(&client).unwrap(), dir.path().join("daemon.sock"));
    }

    /// With `ATUIN_HOME` set, a socket recorded by a daemon that has since exited isn't trusted:
    /// another profile's daemon may be using that path now. The client goes by its own settings,
    /// and refuses the path if something is live there.
    #[rstest]
    fn with_atuin_home_a_stale_pidfile_is_not_trusted(#[values(false, true)] live: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        // A daemon recorded `daemon.sock`, then exited, leaving its pidfile behind. Its path is
        // now live, as if another profile's daemon had taken it.
        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        drop(PidfileGuard::acquire(&daemon.daemon).unwrap());
        let _taken = listen(&dir.path().join("daemon.sock"));

        let client = settings_in(dir.path(), true);
        let path = preferred_path(dir.path(), true);
        let _listener = live.then(|| listen(&path));
        match socket_path(&client) {
            Ok(found) => {
                assert!(!live, "a live socket at {} was accepted", path.display());
                assert_eq!(found, path);
            }
            Err(SocketPathError::InUse { path: in_use, .. }) => {
                assert!(live, "a free socket path was refused");
                assert_eq!(in_use, path);
            }
        }
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
                    assert_eq!(progress, (0, 0));
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
        assert_eq!(client.rebuild_status().await.unwrap(), Some((0, 0)));
        state.send_replace(StoreState::Ready);
        assert_eq!(client.rebuild_status().await.unwrap(), None);
    }
}
