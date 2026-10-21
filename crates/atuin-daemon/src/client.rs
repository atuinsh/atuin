#[cfg(unix)]
use std::borrow::Cow;
use std::num::NonZeroU32;
#[cfg(unix)]
use std::path::Path;

use atuin_client::ai_session::{HarnessKind, HarnessSession, SearchTerms, SessionFilter};
use atuin_client::database::Context;
use atuin_client::history::{History, HistoryId};
#[cfg(unix)]
use atuin_client::settings::Daemon as DaemonSettings;
use atuin_client::settings::{FilterMode, Settings};
use atuin_common::filter::{self, OrFilter};
use atuin_common::range::PyStyleIdxRange;
use easy_cast::Conv;
use eyre::Result;
use futures::{Stream, StreamExt};
use itertools::Itertools;
use time::OffsetDateTime;
use tonic::Code;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tracing::{Level, instrument, span, warn};

use crate::grpc::ai::agent::pb::Session as AiSession;
use crate::grpc::ai::session::pb::ai_session_client::AiSessionClient as AiSessionServiceClient;
use crate::grpc::ai::session::pb::{
    GetSessionEvent, GetSessionRequest, ImportSessionsEvent, ImportSessionsRequest,
    ListSessionsRequest, ProjectSyncedRequest, RebuildSessionsRequest, SearchSessionsMatch,
    SearchSessionsRequest, TailSessionsEvent, TailSessionsRequest,
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
use crate::grpc::versioning::{StrictVersionInterceptor, VersionMatchError, VersionedExt as _};
use crate::output_capture::OutputMatch;
use crate::search::search_client::SearchClient as SearchServiceClient;
use crate::search::{
    FilterMode as RpcFilterMode, PrepareIndexRequest, SearchCommandOutputRequest,
    SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
};
use crate::service::address::{ConnectError, DaemonAddress};
use crate::service::{ActiveService, FindError};

/// The paths that should be checked when looking for existing daemon sockets.
///
/// For the same reasons explained in [`Pidfile::socket_path`][1], when `ATUIN_HOME` is set, this function will
/// only yield the [preferred socket path][0] instead of also including the legacy paths.
///
/// [0]: atuin_client::settings::Daemon::preferred_socket_path
/// [1]: crate::service::pidfile::Pidfile::socket_path
#[cfg(unix)]
pub fn potential_socket_paths(
    daemon: &DaemonSettings,
) -> impl Iterator<Item = Cow<'_, Path>> + use<'_> {
    if atuin_common::dirs::atuin_home_is_set() {
        itertools::Either::Left(std::iter::once(daemon.preferred_socket_path()))
    } else {
        itertools::Either::Right(daemon.potential_socket_paths())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FromSettingsError {
    #[error(transparent)]
    Find(#[from] FindError),
    #[error(transparent)]
    Connect(#[from] ConnectError),
}

/// A connection to the daemon that stamps every request with this client's
/// [`BUILD_STR`](crate::grpc::versioning::BUILD_STR).
type DaemonChannel = InterceptedService<Channel, StrictVersionInterceptor>;

#[derive(Clone)]
pub struct HistoryClient {
    client: HistoryServiceClient<DaemonChannel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonClientErrorKind {
    Connect,
    Unavailable,
    Unimplemented,
    /// The daemon is another build and refused the call.
    BuildMismatch,
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
            // A status with no build isn't a refusal: tonic makes one up when a call never reaches
            // the daemon, and a daemon from before builds were exchanged sends none.
            if let Err(
                mismatch @ (VersionMatchError::Mismatched { .. }
                | VersionMatchError::Malformed { .. }),
            ) = status.check_version()
            {
                warn!(err = ?mismatch, "daemon version mismatch");
                return DaemonClientErrorKind::BuildMismatch;
            }

            return match status.code() {
                Code::Unavailable => DaemonClientErrorKind::Unavailable,
                Code::Unimplemented => DaemonClientErrorKind::Unimplemented,
                _ => DaemonClientErrorKind::OtherGrpc,
            };
        }
    }

    DaemonClientErrorKind::NonGrpc
}

impl From<Channel> for HistoryClient {
    fn from(channel: Channel) -> Self {
        Self {
            client: HistoryServiceClient::with_interceptor(channel, StrictVersionInterceptor),
        }
    }
}

// Wrap the grpc client
impl HistoryClient {
    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        Ok(Self::from(address.into().channel().await?))
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
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

    pub async fn status(&mut self) -> Result<StatusReply, tonic::Status> {
        Ok(self.client.status(StatusRequest {}).await?.into_inner())
    }

    /// Checks the daemon's build against ours. `Err` means the daemon couldn't be asked.
    ///
    /// A daemon of another build refuses `Status`, stamping the refusal with its build. One from
    /// before builds were exchanged answers, unstamped.
    pub async fn check_daemon_version(
        &mut self,
    ) -> Result<Result<(), VersionMatchError>, tonic::Status> {
        match self.client.status(StatusRequest {}).await {
            Ok(response) => Ok(response.check_version()),
            Err(status) => match status.check_version() {
                // Not a refusal: the daemon is our build and failed, or the call never reached it.
                Ok(()) | Err(VersionMatchError::Unannounced) => Err(status),
                mismatch => Ok(mismatch),
            },
        }
    }

    pub async fn tail_history(&mut self) -> Result<tonic::Streaming<TailHistoryReply>> {
        Ok(self.client.tail_history(TailHistoryRequest {}).await?.into_inner())
    }

    pub async fn shutdown(&mut self) -> Result<bool, tonic::Status> {
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
    client: SearchServiceClient<DaemonChannel>,
}

impl From<Channel> for SearchClient {
    fn from(channel: Channel) -> Self {
        Self {
            client: SearchServiceClient::with_interceptor(channel, StrictVersionInterceptor),
        }
    }
}

impl SearchClient {
    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        Ok(Self::from(address.into().channel().await?))
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
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
    /// Relevance-ranked hits for output holding every one of `terms` (a term of several words
    /// as a phrase), each reduced to the lines within `context` of a match, or whole when
    /// `context` is `None`.
    pub async fn search_command_output(
        &mut self,
        terms: Vec<String>,
        limit: Option<NonZeroU32>,
        context: Option<u32>,
    ) -> Result<impl Stream<Item = Result<OutputMatch>> + Send + use<>> {
        let request = SearchCommandOutputRequest {
            terms,
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
    client: AiSessionServiceClient<DaemonChannel>,
}

impl AiClient {
    /// The stub over `channel`, taking messages of any size: a message the daemon streams holds
    /// one captured message, as big as the prompt or reply in it, past tonic's default 4 MiB.
    /// The daemon is local and trusted (the server encodes any size by default).
    fn service(channel: Channel) -> AiSessionServiceClient<DaemonChannel> {
        AiSessionServiceClient::with_interceptor(channel, StrictVersionInterceptor)
            .max_decoding_message_size(usize::MAX)
    }

    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        Ok(Self {
            client: Self::service(address.into().channel().await?),
        })
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
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

    /// Stream the sessions matching `terms` and passing `filter`, most relevant first, at most
    /// `limit` (0 is unbounded). No terms stream them newest first.
    ///
    /// The daemon matches every term as whole words, or with [`SearchTerms::Any`] any term as a
    /// prefix; a [`SearchTerms::Typed`] line (search as you type) is searched as
    /// [`SearchTerms::All`] over its words.
    pub async fn search_sessions(
        &mut self,
        terms: SearchTerms,
        filter: &SessionFilter,
        limit: u32,
    ) -> Result<tonic::Streaming<SearchSessionsMatch>> {
        let (terms, any_term) = match terms {
            SearchTerms::All(terms) => (terms, false),
            SearchTerms::Typed(line) => {
                (line.split_whitespace().map(str::to_owned).collect(), false)
            }
            SearchTerms::Any(terms) => (terms, true),
        };
        let request = SearchSessionsRequest {
            terms,
            limit,
            any_term,
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

    /// Have the daemon project the records this process synced into the record store. Returns
    /// once the projection is over, which goes on if this stops waiting; a failure is only in the
    /// daemon's log.
    pub async fn project_synced(&mut self) -> Result<()> {
        self.client.project_synced(ProjectSyncedRequest {}).await?;
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
    use rstest::rstest;

    use super::*;
    use crate::grpc::versioning::{BUILD_HEADER_KEY, BUILD_STR};

    /// Services return `FAILED_PRECONDITION` for their own reasons too: a refusal is known by the
    /// other build on it.
    #[rstest]
    #[case::other_build(Some("0.0.0"), DaemonClientErrorKind::BuildMismatch)]
    #[case::our_build(Some(BUILD_STR), DaemonClientErrorKind::OtherGrpc)]
    #[case::no_build(None, DaemonClientErrorKind::OtherGrpc)]
    fn a_refusal_is_known_by_its_build(
        #[case] build: Option<&'static str>,
        #[case] expected: DaemonClientErrorKind,
    ) {
        let mut status = tonic::Status::failed_precondition("refused");
        if let Some(build) = build {
            status
                .metadata_mut()
                .insert(BUILD_HEADER_KEY, tonic::metadata::MetadataValue::from_static(build));
        }
        assert_eq!(classify_error(&status.into()), expected);
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
