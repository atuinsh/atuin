use std::num::NonZeroU32;

use atuin_client::database::Context;
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::{FilterMode, Settings};
use atuin_common::filter::{self, OrFilter};
use atuin_common::range::PyStyleIdxRange;
use atuin_common::userio::log::{Message, SINK};
use easy_cast::Conv;
use eyre::Result;
use futures::{Stream, StreamExt};
use itertools::Itertools;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tracing::{Level, instrument, span, warn};

use crate::grpc::history::pb::history_client::HistoryClient as HistoryServiceClient;
use crate::grpc::history::pb::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, CommandCapture, CommandCaptureMeta,
    CompactStoreReply, CompactStoreRequest, DeleteHistoryReply, DeleteHistoryRequest,
    EndHistoryReply, EndHistoryRequest, GetCommandOutputRequest, GetCommandOutputResponse,
    ImportHistoryRequest, LoggedInRequest, RebuildHistoryReply, RebuildHistoryRequest,
    RegisterCommandOutputRequest, ShutdownRequest, StartHistoryReply, StartHistoryRequest,
    StatusReply, StatusRequest, TailHistoryReply, TailHistoryRequest,
};
use crate::grpc::request::ClientInterceptor;
use crate::grpc::userio::pb::ListenLogsRequest;
use crate::grpc::userio::pb::user_io_client::UserIoClient;
use crate::grpc::versioning::{VersionMatchError, VersionedExt as _};
use crate::output_capture::OutputMatch;
use crate::search::search_client::SearchClient as SearchServiceClient;
use crate::search::{
    FilterMode as RpcFilterMode, PrepareIndexRequest, RebuildIndexRequest,
    SearchCommandOutputRequest, SearchContext as RpcSearchContext, SearchRequest, SearchResponse,
};
use crate::service::address::{ConnectError, DaemonAddress};
use crate::service::{ActiveService, FindError};

#[cfg(feature = "ai")]
mod ai;
#[cfg(feature = "ai")]
pub use ai::AiClient;

#[derive(Debug, thiserror::Error)]
pub enum FromSettingsError {
    #[error(transparent)]
    Find(#[from] FindError),
    #[error(transparent)]
    Connect(#[from] ConnectError),
}

/// A connection to the daemon that stamps every request with this client's
/// [`BUILD_STR`](crate::grpc::versioning::BUILD_STR).
type DaemonChannel = InterceptedService<Channel, ClientInterceptor>;

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
            client: HistoryServiceClient::with_interceptor(channel, ClientInterceptor),
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
            timestamp: i64::conv(h.timestamp.as_nanosecond()),
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

    /// Have the daemon import `shell`'s history file, replying straight away unless `wait`, then
    /// once it's imported.
    pub async fn import_history(
        &mut self,
        shell: &str,
        histfile: Option<String>,
        fish_history: Option<String>,
        wait: bool,
    ) -> Result<()> {
        let request = ImportHistoryRequest {
            shell: shell.to_owned(),
            histfile,
            fish_history,
            wait,
        };
        self.client.import_history(request).await?;
        Ok(())
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

    /// Tells the daemon `atuin login` saved a session, so the uploads it holds back for one go now.
    pub async fn logged_in(&mut self) -> Result<(), tonic::Status> {
        self.client.logged_in(LoggedInRequest {}).await?;
        Ok(())
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
            client: SearchServiceClient::with_interceptor(channel, ClientInterceptor),
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

    /// Tell the daemon to rebuild the search index from the history DB.
    pub async fn rebuild_index(&mut self) -> Result<()> {
        self.client.rebuild_index(RebuildIndexRequest {}).await?;
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
            vcs_root: context.vcs_root.map(|path| path.to_string_lossy().to_string()),
        }
    }
}

#[cfg(test)]
mod search_tests {
    use std::path::PathBuf;

    use atuin_domain::record::CmdOrigin;
    use rstest::rstest;

    use crate::client::{
        Context, FilterMode, OrFilter, RpcFilterMode, SearchParams, SearchRequest,
    };

    #[rstest]
    #[case::no_repository(None)]
    #[case::repository(Some("/work/repo"))]
    #[case::unicode_repository(Some("/work/répo"))]
    fn search_request_preserves_vcs_context(#[case] root: Option<&str>) {
        let request = SearchRequest::from(SearchParams {
            query: "cargo build".to_string(),
            query_id: 42,
            filter_mode: FilterMode::Workspace,
            context: Some(Context {
                session: "session".to_string(),
                cwd: "/work/repo/sub".to_string(),
                cmd_origin: CmdOrigin::try_from("host:user").unwrap(),
                host_id: "host-id".to_string(),
                vcs_root: root.map(PathBuf::from),
            }),
            shells: OrFilter::default(),
        });

        assert_eq!(request.query, "cargo build");
        assert_eq!(request.query_id, 42);
        assert_eq!(request.filter_mode, i32::from(RpcFilterMode::Workspace));
        assert!(request.shells.is_empty());
        let context = request.context.unwrap();
        assert_eq!(context.session_id, "session");
        assert_eq!(context.cwd, "/work/repo/sub");
        assert_eq!(context.hostname, "host:user");
        assert_eq!(context.host_id, "host-id");
        assert_eq!(context.vcs_root.as_deref(), root);
    }
}

/// Shows on [`SINK`] what the daemon logs for the user while serving a request, from
/// [`listen`](Self::listen) until [`finish`](Self::finish).
pub(crate) struct LogListener {
    /// Dropped to close this side of the stream, which has the daemon send the messages it has
    /// left and end it.
    close: Option<oneshot::Sender<()>>,
    shown: JoinHandle<()>,
}

impl LogListener {
    /// Listens to the daemon over `channel` for the request of the current
    /// [`scope`](crate::grpc::request::scope),
    /// showing every message it logs for it from when this returns on.
    pub(crate) async fn listen(channel: Channel) -> Result<Self, tonic::Status> {
        let (close, closed) = oneshot::channel::<()>();
        let requests = futures::stream::once(closed)
            .filter_map(|_| std::future::ready(None::<ListenLogsRequest>));
        let mut replies = UserIoClient::with_interceptor(channel, ClientInterceptor)
            .listen_logs(requests)
            .await?
            .into_inner();

        // Runs until the daemon ends the stream, which dropping `close` asks for.
        let shown = tokio::spawn(async move {
            while let Ok(Some(reply)) = replies.message().await {
                let Some(message) = reply.message else {
                    continue;
                };
                match Message::try_from(message) {
                    Ok(message) => SINK.show(message),
                    Err(err) => warn!(?err, "dropped a message from the daemon"),
                }
            }
        });

        Ok(Self {
            close: Some(close),
            shown,
        })
    }

    /// Shows the messages the daemon has logged until now, then stops listening.
    pub(crate) async fn finish(mut self) {
        /// How long the daemon has to send what it queued for the request.
        const FINISH_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

        drop(self.close.take());
        // One that panicked has shown what it could, and one still running is aborted on drop.
        let _ = tokio::time::timeout(FINISH_WAIT, &mut self.shown).await;
    }
}

impl Drop for LogListener {
    fn drop(&mut self) {
        self.shown.abort();
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
