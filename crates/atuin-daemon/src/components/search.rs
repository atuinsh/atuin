//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library
//! with frecency-based ranking and dynamic filtering.

use std::ops::Deref;
use std::{pin::Pin, sync::Arc};

use atuin_client::database::Database;
use atuin_common::path::DisplayRichExt;
use eyre::Result;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, debug, error, info, instrument, span, trace};
use uuid::Uuid;

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    search::{
        FilterMode, IndexFilterMode, SearchIndex, SearchRequest, SearchResponse, ShellFilter,
        search_server::{Search as SearchSvc, SearchServer},
    },
};

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
    index().await.rebuild_frecency(&settings.search).await;
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

#[tonic::async_trait]
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
            index.write().await.shells = ShellFilter::from_initial_settings(
                &handle_for_loader.settings().await.search.shells,
            );
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
    async fn maybe_rebuild_index(&self, shells: Vec<String>) -> Result<Option<SearchIndex>, ()> {
        let Some(new_filter) = self.index.read().await.shells.update(shells) else {
            return Ok(None);
        };

        info!("Rebuilding search index from database after shell filter change");

        let new_index = SearchIndex::new(new_filter);
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

                let index = match this.maybe_rebuild_index(search_req.shells).await {
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
                let history_ids = span!(Level::TRACE, "daemon_search_query", %query, query_id)
                    .in_scope(|| async { index.search(&query, index_filter, RESULTS_LIMIT).await })
                    .await;
                drop(index);

                // Convert history IDs to bytes
                let ids: Vec<Vec<u8>> = history_ids
                    .iter()
                    .filter_map(|id| {
                        Uuid::parse_str(id)
                            .ok()
                            .map(|uuid| uuid.as_bytes().to_vec())
                    })
                    .collect();

                if tx.send(Ok(SearchResponse { query_id, ids })).await.is_err() {
                    break; // Client disconnected
                }
            }
        });

        // Convert receiver to stream
        let out_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(out_stream)))
    }
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
