//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library
//! with frecency-based ranking and dynamic filtering.

use std::ops::Deref;
use std::{pin::Pin, sync::Arc};

use atuin_client::database::Database;
use atuin_common::filter::OrFilter;
use atuin_common::path::DisplayRichExt;
use eyre::Result;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, debug, error, info, instrument, span, trace};

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    search::{
        FilterMode, IndexFilterMode, PrepareIndexRequest, PrepareIndexResponse, SearchIndex,
        SearchRequest, SearchResponse, SuggestReply, SuggestRequest, SuggestScope, Suggestion,
        search_server::{Search as SearchSvc, SearchServer},
    },
};

/// Cap on suggestions returned per request, whatever the client asks for.
const SUGGEST_LIMIT: u32 = 50;

const PAGE_SIZE: usize = 5000;
const RESULTS_LIMIT: u32 = 200;
/// How often to rebuild the frecency map (in seconds).
const FRECENCY_REFRESH_INTERVAL_SECS: u64 = 60;

/// Build the search index without building the frecency map.
///
/// `index` is a closure to support both shared `RwLock` indices and owned indices:
///
/// * Owned: `async || &my_owned_index`
/// * Shared: `|| my_rwlock_index.read()`
///
/// In the shared case, this ensures that the lock isn't held while this function does expensive
/// computation.
#[instrument(skip_all, level = Level::TRACE)]
async fn build_index_only<F, R>(index: F, handle: &DaemonHandle) -> Result<(), ()>
where
    F: Fn() -> R,
    R: Future<Output: Deref<Target = SearchIndex>>,
{
    info!(
        "Loading history into search index; page size = {}",
        PAGE_SIZE
    );
    let db = handle.history_db();
    let mut pager = db.all_paged(PAGE_SIZE, false, true);
    loop {
        match pager.next().await {
            Ok(Some(histories)) => {
                info!(
                    "Loading {} history entries into search index",
                    histories.len()
                );
                index().await.add_histories(&histories);
                // Indexing a page is CPU-bound, and the daemon runs on a
                // current-thread runtime: without a scheduling point here,
                // a rebuild holds the only thread from first page to last
                // and every request waits it out. The pty-proxy's popup
                // gives up after 100ms, so for it that means no suggestions
                // at all until the rebuild finishes.
                tokio::task::yield_now().await;
            }
            Ok(None) => {
                info!(
                    "History load complete; {} unique commands indexed",
                    index().await.command_count()
                );
                return Ok(());
            }
            Err(e) => {
                error!("Failed to load history: {}", e);
                return Err(());
            }
        }
    }
}

/// Build the frecency map.
///
/// `index` is a closure to support both shared `RwLock` indices and owned indices:
///
/// * Owned: `async || &my_owned_index`
/// * Shared: `|| my_rwlock_index.read()`
///
/// In the shared case, this ensures that the lock isn't held while this function does expensive
/// computation.
#[instrument(skip_all, level = Level::TRACE)]
async fn build_frecency<F, R>(index: F, handle: &DaemonHandle)
where
    F: Fn() -> R,
    R: Future<Output: Deref<Target = SearchIndex>>,
{
    let settings = handle.settings().await;
    index().await.rebuild_frecency(&settings.search);
    info!("Frecency map built");
}

/// Build the search index and frecency map.
///
/// `index` is a closure to support both shared `RwLock` indices and owned indices:
///
/// * Owned: `async || &my_owned_index`
/// * Shared: `|| my_rwlock_index.read()`
///
/// In the shared case, this ensures that the lock isn't held while this function does expensive
/// computation.
async fn build_index<F, R>(index: F, handle: &DaemonHandle) -> Result<(), ()>
where
    F: Fn() -> R,
    R: Future<Output: Deref<Target = SearchIndex>>,
{
    build_index_only(&index, handle).await?;
    build_frecency(index, handle).await;
    Ok(())
}

/// Search component - provides fuzzy search over command history.
///
/// This component:
/// - Maintains a deduplicated search index with frecency ranking
/// - Loads history from the database on startup
/// - Updates the index when history events occur
/// - Provides the Search gRPC service
pub struct SearchComponent {
    index: Arc<RwLock<SearchIndex>>,
    handle: Option<DaemonHandle>,
    loader_handle: Option<tokio::task::JoinHandle<()>>,
    frecency_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SearchComponent {
    /// Create a new search component.
    pub fn new() -> Self {
        Self {
            index: Arc::new(RwLock::new(SearchIndex::default())),
            handle: None,
            loader_handle: None,
            frecency_handle: None,
        }
    }

    /// Get the gRPC service for this component.
    pub fn grpc_service(&self) -> SearchGrpcServiceBuilder {
        SearchGrpcServiceBuilder {
            index: self.index.clone(),
        }
    }

    /// Rebuild the entire search index from the database without updating the frecency map.
    async fn rebuild_index_only(&self) {
        let Some(handle) = self.handle.as_ref() else {
            error!("Component not initialized");
            return;
        };
        info!("Rebuilding search index from database");

        // Create a new index
        let new_index = SearchIndex::new(self.index.read().await.shells.clone());
        if build_index_only(async || &new_index, handle).await.is_err() {
            return;
        }

        info!(
            "Search index rebuild complete; {} unique commands",
            new_index.command_count()
        );
        *self.index.write().await = new_index;
    }
}

impl Default for SearchComponent {
    fn default() -> Self {
        Self::new()
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
            // Build the initial shell filter from the settings. If `search.shells` is "auto", this
            // will use the value of the `ATUIN_SHELL` environment variable. This variable might be
            // correct if the daemon was autostarted by the shell hooks, but if it's unset or
            // incorrect, we'll simply rebuild the index upon receipt of the first request.
            let shells = handle_for_loader
                .settings()
                .await
                .search
                .shells
                .to_filter()
                .to_vec_filter();
            index.write().await.shells = shells;
            let _ = build_index(|| index.read(), &handle_for_loader).await;
        }));

        // Spawn background task to periodically refresh frecency
        let index_for_frecency = self.index.clone();
        self.frecency_handle = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                FRECENCY_REFRESH_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;
                trace!("Refreshing frecency map");
                build_frecency(|| index_for_frecency.read(), &handle).await;
            }
        }));

        tracing::info!("search component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        match event {
            DaemonEvent::HistorySynced(ids) => {
                debug!(count = ids.len(), "Indexing synced history entries");

                let Some(handle) = self.handle.as_ref() else {
                    return Ok(());
                };

                let histories = handle.history_db().load_active(ids).await?;
                self.index.read().await.add_histories(&histories);
            }
            DaemonEvent::HistoryStarted(history) => {
                debug!(id = %history.id, command = %history.command, "History started (no index action)");
            }
            DaemonEvent::HistoryEnded(history) => {
                span!(Level::TRACE, "inject_history_ended")
                    .in_scope(async || {
                        self.index.read().await.add_history(history);
                    })
                    .await;
            }
            DaemonEvent::HistoryPruned | DaemonEvent::HistoryRebuilt => {
                info!("History store pruned or rebuilt, rebuilding search index");
                self.rebuild_index_only().await;
            }
            DaemonEvent::HistoryDeleted { ids } => {
                info!(
                    count = ids.len(),
                    "History deleted, rebuilding search index"
                );
                // For now, just rebuild the entire index. A more efficient implementation
                // would remove specific items from the index.
                self.rebuild_index_only().await;
            }
            DaemonEvent::SettingsReloaded => {
                if let Some(handle) = self.handle.as_ref() {
                    info!("Rebuilding frecency map after settings update");
                    build_frecency(|| self.index.read(), handle).await;
                }
            }
            // Events we don't care about
            DaemonEvent::SyncCompleted { .. }
            | DaemonEvent::SyncFailed { .. }
            | DaemonEvent::ForceSync
            | DaemonEvent::ShutdownRequested => {}
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
    index: Arc<RwLock<SearchIndex>>,
}

impl SearchGrpcServiceBuilder {
    pub fn build(self, handle: DaemonHandle) -> SearchServer<SearchGrpcService> {
        SearchServer::new(SearchGrpcService {
            index: self.index,
            handle,
        })
    }
}

/// The gRPC service implementation.
#[derive(Clone)]
pub struct SearchGrpcService {
    index: Arc<RwLock<SearchIndex>>,
    handle: DaemonHandle,
}

impl SearchGrpcService {
    async fn maybe_rebuild_index(
        &self,
        shells: OrFilter<Vec<String>>,
    ) -> Result<Option<SearchIndex>, ()> {
        if self.index.read().await.shells == shells {
            return Ok(None);
        }

        info!("Rebuilding search index from database after shell filter change");

        let new_index = SearchIndex::new(shells);
        build_index(async || &new_index, &self.handle).await?;

        info!(
            "Search index rebuild complete; {} unique commands",
            new_index.command_count()
        );
        Ok(Some(new_index))
    }
}

#[tonic::async_trait]
impl SearchSvc for SearchGrpcService {
    type SearchStream = Pin<Box<dyn Stream<Item = Result<SearchResponse, Status>> + Send>>;

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
                let filter_mode: FilterMode = search_req
                    .filter_mode
                    .try_into()
                    .unwrap_or(FilterMode::Global);
                let proto_context = search_req.context;

                debug!(
                    "search request: query = {}, query_id = {}, filter_mode = {}, context = {:?}",
                    query,
                    query_id,
                    filter_mode.as_str_name(),
                    proto_context
                );

                // Convert proto FilterMode + context to IndexFilterMode
                let index_filter = convert_filter_mode(filter_mode, &proto_context);

                // An empty list in `SearchRequest::shells` means "all".
                let shells = OrFilter::from_list(search_req.shells).unwrap_or_default();
                let index = match this.maybe_rebuild_index(shells).await {
                    Ok(Some(new_index)) => {
                        let mut guard = this.index.write().await;
                        *guard = new_index;
                        guard.downgrade()
                    }
                    Ok(None) => this.index.read().await,
                    Err(()) => {
                        let _ = tx
                            .send(Err(Status::internal("failed to build index")))
                            .await;
                        break;
                    }
                };

                // Perform the search
                let history_ids: Vec<Vec<u8>> =
                    span!(Level::TRACE, "daemon_search_query", %query, query_id).in_scope(|| {
                        index
                            .search(&query, index_filter, RESULTS_LIMIT)
                            .map(Vec::from)
                            .collect()
                    });
                drop(index);

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
        if let Some(index) = self
            .maybe_rebuild_index(shells)
            .await
            .map_err(|()| Status::internal("failed to build index"))?
        {
            *self.index.write().await = index;
        }
        Ok(Response::new(PrepareIndexResponse {}))
    }

    #[instrument(skip_all, level = Level::TRACE, name = "suggest_rpc")]
    async fn suggest(
        &self,
        request: Request<SuggestRequest>,
    ) -> Result<Response<SuggestReply>, Status> {
        let request = request.into_inner();
        let limit = request.limit.min(SUGGEST_LIMIT) as usize;

        // Served from whatever index the daemon currently holds, and never
        // from a rebuilt one. `search` rebuilds when the requested shell
        // filter differs from the index's, which reads the whole history
        // database — seconds on an established history. That is fine for an
        // interactive search, which waits as long as it takes, and fatal
        // here: this caller gives up after 100ms, and abandoning the request
        // takes the rebuild with it, so the next keystroke starts over and
        // suggestions never arrive at all.
        //
        // The cost of not rebuilding is that suggestions ride whichever
        // shell filter the index was built with. Usually that is the user's
        // shell already; when it is broader, they see commands from another
        // shell of theirs until an interactive search narrows it.
        let index = self.index.read().await;

        let (directory, workspace) = suggest_scope_paths(request.context.as_ref());
        // Directory ranking fails silently by design — a directory nobody has
        // run anything in simply ranks nothing — so the directory it ranked
        // against is the one thing worth naming when suggestions look wrong.
        trace!(
            directory = directory.as_deref().unwrap_or("<none>"),
            workspace = workspace.as_deref().unwrap_or("<none>"),
            "ranking suggestions"
        );

        let suggestions = index
            .suggest(
                &request.query,
                limit,
                SuggestScope {
                    directory: directory.as_deref(),
                    workspace: workspace.as_deref(),
                    filter_failed: request.filter_failed,
                },
            )
            .into_iter()
            .map(|command| Suggestion { command })
            .collect();
        Ok(Response::new(SuggestReply { suggestions }))
    }
}

/// The directory and workspace `Suggest` ranks by, normalized the way
/// [`CommandData`](crate::search::SearchIndex) interns a history entry's
/// `cwd` — the same preparation `convert_filter_mode` gives these two paths.
/// A path that misses that normalization interns to nothing and silently
/// ranks every command as "elsewhere", so it is worth its own seam.
fn suggest_scope_paths(
    context: Option<&crate::search::SearchContext>,
) -> (Option<String>, Option<String>) {
    let directory = context.map(|ctx| ctx.cwd.display_rich().trailing_slash(true).to_string());
    let workspace = context
        .and_then(|ctx| ctx.git_root.as_ref())
        .map(|root| root.display_rich().trailing_slash(true).to_string());
    (directory, workspace)
}

/// Convert proto FilterMode and context to IndexFilterMode.
fn convert_filter_mode(
    mode: FilterMode,
    context: &Option<crate::search::SearchContext>,
) -> IndexFilterMode {
    match (mode, context) {
        (FilterMode::Global, _) => IndexFilterMode::Global,
        (FilterMode::Directory, Some(ctx)) => {
            IndexFilterMode::Directory(ctx.cwd.display_rich().trailing_slash(true).to_string())
        }
        (FilterMode::Workspace, Some(ctx)) => {
            if let Some(ref git_root) = ctx.git_root {
                IndexFilterMode::Workspace(git_root.display_rich().trailing_slash(true).to_string())
            } else {
                // Fall back to directory if no git root
                IndexFilterMode::Directory(ctx.cwd.display_rich().trailing_slash(true).to_string())
            }
        }
        (FilterMode::Host, Some(ctx)) => IndexFilterMode::Host(ctx.hostname.clone()),
        (FilterMode::Session, Some(ctx)) => IndexFilterMode::Session(ctx.session_id.clone()),
        (FilterMode::SessionPreload, Some(ctx)) => {
            // SessionPreload is similar to Session - filter by session
            IndexFilterMode::Session(ctx.session_id.clone())
        }
        // If no context provided, fall back to global
        _ => IndexFilterMode::Global,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{SearchContext, SuggestScope};
    use atuin_client::history::History;
    use time::macros::datetime;

    fn history_in(command: &str, cwd: &str) -> History {
        History::import()
            .timestamp(datetime!(2024-01-01 10:00 UTC))
            .command(command)
            .cwd(cwd)
            .build()
            .into()
    }

    /// The path a client sends is a plain cwd, exactly as the shell reports
    /// it; the index interns directories with a trailing separator. If those
    /// two ever drift apart, directory ranking stops working with nothing to
    /// show for it — every command simply ranks as "elsewhere".
    #[test]
    fn a_client_cwd_ranks_against_the_directories_the_index_interned() {
        let index = SearchIndex::default();
        index.add_history(&history_in("cargo build --release", "/home/user/elsewhere"));
        index.add_history(&history_in("cargo test", "/home/user/repo/crates"));
        index.add_history(&history_in("cargo clippy --fix", "/home/user/repo"));

        // What the pty-proxy sends: the shell's cwd and its git root, both
        // unadorned.
        let context = SearchContext {
            session_id: String::new(),
            cwd: "/home/user/repo".to_string(),
            hostname: String::new(),
            host_id: String::new(),
            git_root: Some("/home/user/repo".to_string()),
        };
        let (directory, workspace) = suggest_scope_paths(Some(&context));

        let results = index.suggest(
            "cargo",
            10,
            SuggestScope {
                directory: directory.as_deref(),
                workspace: workspace.as_deref(),
                filter_failed: true,
            },
        );
        assert_eq!(
            results,
            vec!["cargo clippy --fix".to_string()],
            "only what ran in this exact directory survives: `cargo test` ran \
             in a sibling crate of the same workspace, which is a different \
             set of files"
        );
    }

    /// A client that sends no context at all still gets suggestions, just
    /// without locality.
    #[test]
    fn a_missing_context_ranks_everything_as_elsewhere() {
        let (directory, workspace) = suggest_scope_paths(None);
        assert!(directory.is_none());
        assert!(workspace.is_none());
    }
}
