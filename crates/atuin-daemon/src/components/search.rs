//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library
//! with frecency-based ranking and dynamic filtering.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_client::settings::Settings;
use atuin_common::filter::OrFilter;
use atuin_common::path::DisplayRichExt;
use atuin_common::shell::Shell;
use eyre::Result;
use futures::StreamExt;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Instrument as _, Level, debug, error, info, instrument, span, trace, warn};

use crate::daemon::{Component, DaemonHandle};
use crate::events::DaemonEvent;
use crate::output_capture::OutputStore;
use crate::search::search_server::{Search as SearchSvc, SearchServer};
use crate::search::{
    FilterMode, FrecencyMultipliers, IndexFilterMode, OutputSearchLine, OutputSearchMatch,
    PrepareIndexRequest, PrepareIndexResponse, RebuildIndexRequest, RebuildIndexResponse,
    RebuiltIndex, SearchCommandOutputRequest, SearchIndex, SearchIndexSettings, SearchRequest,
    SearchResponse, SharedSearchIndex,
};

const RESULTS_LIMIT: usize = 200;
/// How often to rebuild the frecency map (in seconds).
const FRECENCY_REFRESH_INTERVAL_SECS: u64 = 60;
/// How long daemon startup waits for [`startup_shell`] before falling back to an "all" filter.
const STARTUP_SHELL_TIMEOUT: Duration = Duration::from_millis(500);

/// Search component - provides fuzzy search over command history.
///
/// This component:
/// - Maintains a deduplicated search index with frecency ranking
/// - Loads history from the database on startup
/// - Updates the index when history events occur
/// - Provides the Search gRPC service
pub struct SearchComponent {
    index: Arc<SharedSearchIndex>,
    handle: Option<DaemonHandle>,
    loader_handle: Option<tokio::task::JoinHandle<()>>,
    frecency_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SearchComponent {
    /// Create a new search component whose index starts out with the shells that `settings`
    /// selects.
    pub async fn new(settings: Arc<Settings>) -> Self {
        let frecency_multipliers = FrecencyMultipliers::from(&settings.search);
        // Build the initial shell filter from the settings. `startup_shell` can block, so run it on
        // a blocking thread with a timeout.
        //
        // This is a best-effort guess: if our guess is wrong, the first search request (or
        // prepare_index from the shell hooks) will rebuild the index with the right filter.
        let get_shell_filter = tokio::task::spawn_blocking(move || {
            settings.search.shells.to_filter_with(startup_shell).to_vec_filter()
        });

        let shells = match tokio::time::timeout(STARTUP_SHELL_TIMEOUT, get_shell_filter).await {
            Ok(Ok(shells)) => shells,
            Ok(Err(err)) => {
                error!(?err, "building the startup shell filter panicked");
                OrFilter::all()
            }
            Err(_) => {
                warn!("timed out looking up the login shell; indexing commands from every shell");
                OrFilter::all()
            }
        };

        Self::with_settings(SearchIndexSettings {
            frecency_multipliers,
            shells,
        })
    }

    fn with_settings(settings: SearchIndexSettings) -> Self {
        Self {
            index: Arc::new(SharedSearchIndex::unfilled(settings)),
            handle: None,
            loader_handle: None,
            frecency_handle: None,
        }
    }

    /// Get the gRPC service for this component.
    #[must_use]
    pub fn grpc_service(&self, output_store: Arc<OutputStore>) -> SearchGrpcServiceBuilder {
        SearchGrpcServiceBuilder {
            index: self.index.clone(),
            output_store,
        }
    }

    /// Get a shared handle to the live search index.
    #[must_use]
    pub const fn index(&self) -> &Arc<SharedSearchIndex> {
        &self.index
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn rebuild_frecency(handle: &DaemonHandle, index: &SharedSearchIndex) {
        let multipliers = FrecencyMultipliers::from(&handle.settings().await.search);
        index.rebuild_frecency(multipliers);
    }

    /// Populate the search index from the database.
    ///
    /// The pages are added to the installed index as they load. Searches during the load see the
    /// history that has loaded so far. See [`SharedSearchIndex::fill`].
    #[instrument(skip_all, level = Level::TRACE)]
    async fn populate_index(
        handle: &DaemonHandle,
        index: &SharedSearchIndex,
    ) -> Result<(), eyre::Error> {
        info!(
            "Loading history into search index; page size = {}",
            SearchIndex::HISTORY_LOAD_PAGE_SIZE
        );
        index
            .fill(handle.history_db())
            .await
            .inspect_err(|e| error!("Failed to load history: {e}"))?;
        info!("History load complete; {} unique commands indexed", index.read().command_count());
        Ok(())
    }
}

/// The best guess at the shell that `search.shells = "auto"` refers to when the daemon starts.
///
/// If the daemon was autostarted by the shell hooks, we can use the value of `ATUIN_SHELL`.
/// Otherwise (e.g., if started by systemd or launchd), we use the user's login shell as a best
/// guess.
///
/// Looking up the login shell requires running a command, so this function blocks.
fn startup_shell() -> Option<String> {
    if let Ok(shell) = atuin_common::env::var("ATUIN_SHELL")
        && !shell.is_empty()
    {
        debug!(shell, "using ATUIN_SHELL for the initial search index");
        return Some(shell);
    }

    let login_shell = Shell::default_shell()
        .inspect_err(|err| info!("could not look up the login shell: {err}"))
        .ok()?;
    let Some(shell) = login_shell.hook_name() else {
        info!(%login_shell, "atuin has no hooks for the login shell");
        return None;
    };
    debug!(shell, "using the login shell for the initial search index");
    Some(shell.to_owned())
}

impl Default for SearchComponent {
    /// A search component whose index uses the [default settings](SearchIndexSettings::default).
    fn default() -> Self {
        Self::with_settings(SearchIndexSettings::default())
    }
}

impl Component for SearchComponent {
    fn name(&self) -> &'static str {
        "search"
    }

    async fn start(&mut self, handle: DaemonHandle) -> Result<()> {
        self.handle = Some(handle.clone());

        // Spawn background task to load history into index
        let index = self.index.clone();
        let handle_for_loader = handle.clone();

        self.loader_handle = Some(tokio::spawn(async move {
            let _ = Self::populate_index(&handle_for_loader, &index).await.inspect_err(|err| {
                error!(?err, "failed to rebuild the index.");
            });
        }));

        // Spawn background task to periodically refresh frecency
        let handle_for_frecency = handle;
        let index_for_frecency = self.index.clone();
        self.frecency_handle = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                FRECENCY_REFRESH_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;
                trace!("Refreshing frecency map");
                Self::rebuild_frecency(&handle_for_frecency, &index_for_frecency).await;
            }
        }));

        tracing::info!("search component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        match event {
            DaemonEvent::SettingsReloaded => {
                if let Some(handle) = self.handle.as_ref() {
                    info!("Rebuilding frecency map after settings update");
                    Self::rebuild_frecency(handle, &self.index).await;
                }
            }
            // Events we don't care about
            DaemonEvent::ShutdownRequested | DaemonEvent::LoggedIn => {}
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.loader_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.frecency_handle.take() {
            handle.abort();
        }
        tracing::info!("search component stopped");
        Ok(())
    }
}

pub struct SearchGrpcServiceBuilder {
    index: Arc<SharedSearchIndex>,
    output_store: Arc<OutputStore>,
}

impl SearchGrpcServiceBuilder {
    #[must_use]
    pub fn build(self, handle: DaemonHandle) -> SearchServer<SearchGrpcService> {
        SearchServer::new(SearchGrpcService {
            index: self.index,
            output_store: self.output_store,
            handle,
        })
    }
}

/// The gRPC service implementation.
#[derive(Clone)]
pub struct SearchGrpcService {
    index: Arc<SharedSearchIndex>,
    output_store: Arc<OutputStore>,
    handle: DaemonHandle,
}

impl SearchGrpcService {
    /// Return an index that has `shells` as its shell filter.
    ///
    /// If the installed index has a different filter, this rebuilds it first. See
    /// [`SharedSearchIndex::rebuild`].
    async fn maybe_rebuild_index(
        &self,
        shells: &OrFilter<Vec<String>>,
    ) -> Result<RebuiltIndex<'_>, ()> {
        self.index
            .rebuild(self.handle.history_db(), |settings| {
                (settings.installed.shells != *shells).then(|| SearchIndexSettings {
                    shells: shells.clone(),
                    ..settings.latest.clone()
                })
            })
            .await
            .map_err(|e| error!("Failed to rebuild search index: {e}"))
    }

    /// Search an index that has `shells` as its shell filter, and return the matching history ids.
    ///
    /// The index is rebuilt first if its filter is different.
    async fn search_index(
        &self,
        query: &str,
        shells: &OrFilter<Vec<String>>,
        filter: &IndexFilterMode,
    ) -> Result<Vec<Vec<u8>>, ()> {
        fn ids(ids: impl Iterator<Item = HistoryId>) -> Vec<Vec<u8>> {
            ids.map(|id| id.into_bytes().to_vec()).collect()
        }

        // Search the index that the rebuild check returned. That index is guaranteed to have this
        // request's shell filter. Each index is dropped before the next await, because it may hold
        // the index's read lock.
        let pending = {
            let index = self.maybe_rebuild_index(shells).await?;
            match index.search(query, filter, RESULTS_LIMIT) {
                Ok(results) => return Ok(ids(results)),
                Err(pending) => pending,
            }
        };

        // The search needs data from the database. Load that data without holding the index's
        // lock.
        let loaded = pending.load(self.handle.history_db()).await;
        let index = self.maybe_rebuild_index(shells).await?;
        Ok(ids(index.finish_search(loaded)))
    }
}

#[tonic::async_trait]
impl SearchSvc for SearchGrpcService {
    type SearchStream = Pin<Box<dyn Stream<Item = Result<SearchResponse, Status>> + Send>>;
    type SearchCommandOutputStream =
        Pin<Box<dyn Stream<Item = Result<OutputSearchMatch, Status>> + Send>>;

    #[instrument(skip_all, level = Level::TRACE, name = "search_rpc")]
    async fn search(
        &self,
        request: Request<Streaming<SearchRequest>>,
    ) -> Result<Response<Self::SearchStream>, Status> {
        let mut in_stream = request.into_inner();
        let this = self.clone();

        // Create output channel
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<SearchResponse, Status>>(128);

        // Spawn task to handle incoming requests and send responses
        tokio::spawn(async move {
            while let Some(req) = in_stream.message().await.transpose() {
                let search_req = match req {
                    Ok(req) => req,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                };

                let query = search_req.query;
                let query_id = search_req.query_id;
                let filter_mode: FilterMode =
                    search_req.filter_mode.try_into().unwrap_or(FilterMode::Global);
                let proto_context = search_req.context;

                debug!(
                    "search request: query = {}, query_id = {}, filter_mode = {}, context = {:?}",
                    query,
                    query_id,
                    filter_mode.as_str_name(),
                    proto_context
                );

                // Convert proto FilterMode + context to IndexFilterMode
                let index_filter = convert_filter_mode(filter_mode, proto_context.as_ref());

                // An empty list in `SearchRequest::shells` means "all".
                let shells = OrFilter::from_list(search_req.shells).unwrap_or_default();
                let history_ids = this
                    .search_index(&query, &shells, &index_filter)
                    .instrument(span!(Level::TRACE, "daemon_search_query", %query, query_id))
                    .await;
                let Ok(history_ids) = history_ids else {
                    let _ = tx.send(Err(Status::internal("failed to build index"))).await;
                    break;
                };

                if tx
                    .send(Ok(SearchResponse {
                        query_id,
                        ids: history_ids,
                    }))
                    .await
                    .is_err()
                {
                    break; // Client disconnected
                }
            }
        });

        // Convert receiver to stream
        let out_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(out_stream)))
    }

    async fn prepare_index(
        &self,
        request: Request<PrepareIndexRequest>,
    ) -> Result<Response<PrepareIndexResponse>, Status> {
        // Same as `SearchRequest::shells` -- empty list means "all".
        let shells = OrFilter::from_list(request.into_inner().shells).unwrap_or_default();
        let this = self.clone();
        // The rebuild runs in its own task. That way a client disconnect can't cancel it.
        tokio::spawn(
            async move { this.maybe_rebuild_index(&shells).await.map(drop) }
                .instrument(tracing::Span::current()),
        )
        .await
        .map_err(|e| Status::internal(format!("index build did not complete: {e}")))?
        .map_err(|()| Status::internal("failed to build index"))?;
        Ok(Response::new(PrepareIndexResponse {}))
    }

    async fn rebuild_index(
        &self,
        _request: Request<RebuildIndexRequest>,
    ) -> Result<Response<RebuildIndexResponse>, Status> {
        let index = self.index.clone();
        let handle = self.handle.clone();

        // Using `tokio::spawn` so that a client that stops waiting for the response doesn't cancel
        // the rebuild.
        tokio::spawn(
            async move {
                let frecency_multipliers =
                    FrecencyMultipliers::from(&handle.settings().await.search);
                let rebuilt = index
                    .rebuild(handle.history_db(), |settings| {
                        Some(SearchIndexSettings {
                            frecency_multipliers,
                            ..settings.latest.clone()
                        })
                    })
                    .await?;
                drop(rebuilt);
                Ok::<_, eyre::Report>(())
            }
            .instrument(tracing::Span::current()),
        )
        .await
        .map_err(eyre::Report::from)
        .and_then(std::convert::identity)
        .map_err(|e| {
            error!("Failed to rebuild search index: {e:#}");
            Status::internal("failed to rebuild the search index")
        })?;

        Ok(Response::new(RebuildIndexResponse {}))
    }

    #[instrument(skip_all, level = Level::TRACE, name = "search_command_output_rpc")]
    async fn search_command_output(
        &self,
        request: Request<SearchCommandOutputRequest>,
    ) -> Result<Response<Self::SearchCommandOutputStream>, Status> {
        let request = request.into_inner();

        // 0 means unbounded: the index streams results lazily by relevance, so the client can
        // consume what it needs and drop the stream. There is no server-side ceiling here.
        let limit = usize::try_from(request.limit).unwrap_or(0);
        let context = request.context.map(|c| usize::try_from(c).unwrap_or(usize::MAX));

        let matches = self
            .output_store
            .search(request.terms.iter().map(String::as_str), limit, context)
            .await
            .items()
            .map(|result| match result {
                Ok(m) => Ok(OutputSearchMatch {
                    history_id: Some(m.history_id.into()),
                    lines: m
                        .lines
                        .iter()
                        .map(|l| OutputSearchLine {
                            line: l.line,
                            content: Some((&l.content).into()),
                        })
                        .collect(),
                    score: m.score,
                }),
                Err(err) => {
                    error!(?err, "output full-text search failed");
                    Err(Status::internal("output search failed"))
                }
            });

        Ok(Response::new(Box::pin(matches)))
    }
}

/// Convert proto FilterMode and context to IndexFilterMode.
fn convert_filter_mode(
    mode: FilterMode,
    context: Option<&crate::search::SearchContext>,
) -> IndexFilterMode {
    match (mode, context) {
        (FilterMode::Global, _) => IndexFilterMode::Global,
        (FilterMode::Directory, Some(ctx)) => {
            IndexFilterMode::Directory(ctx.cwd.display_rich().trailing_slash(true).to_string())
        }
        (FilterMode::Workspace, Some(ctx)) => {
            if let Some(ref vcs_root) = ctx.vcs_root {
                IndexFilterMode::Workspace(vcs_root.display_rich().trailing_slash(true).to_string())
            } else {
                // Fall back to directory if no repo root
                IndexFilterMode::Directory(ctx.cwd.display_rich().trailing_slash(true).to_string())
            }
        }
        (FilterMode::Host, Some(ctx)) => IndexFilterMode::Host(ctx.hostname.clone()),
        (FilterMode::Session, Some(ctx)) => IndexFilterMode::Session(ctx.session_id.clone()),
        (FilterMode::SessionPreload, Some(ctx)) => {
            IndexFilterMode::SessionPreload(ctx.session_id.clone())
        }
        // If no context provided, fall back to global
        _ => IndexFilterMode::Global,
    }
}
