//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library
//! with frecency-based ranking and dynamic filtering.

use std::{pin::Pin, sync::Arc};

use atuin_client::database::Database;
use eyre::Result;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, debug, info, instrument, span, trace};
use uuid::Uuid;

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    search::{
        FilterMode, IndexFilterMode, QueryContext, SearchIndex, SearchRequest, SearchResponse,
        search_server::{Search as SearchSvc, SearchServer},
    },
};

const PAGE_SIZE: usize = 5000;
const RESULTS_LIMIT: u32 = 200;
/// How often to rebuild the frecency map (in seconds).
const FRECENCY_REFRESH_INTERVAL_SECS: u64 = 60;

/// Search component - provides fuzzy search over command history.
///
/// This component:
/// - Maintains a deduplicated search index with frecency ranking
/// - Loads history from the database on startup
/// - Updates the index when history events occur
/// - Provides the Search gRPC service
pub struct SearchComponent {
    index: Arc<RwLock<SearchIndex>>,
    handle: tokio::sync::RwLock<Option<DaemonHandle>>,
    loader_handle: Option<tokio::task::JoinHandle<()>>,
    frecency_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SearchComponent {
    /// Create a new search component.
    pub fn new() -> Self {
        Self {
            index: Arc::new(RwLock::new(SearchIndex::new())),
            handle: tokio::sync::RwLock::new(None),
            loader_handle: None,
            frecency_handle: None,
        }
    }

    /// Get the gRPC service for this component.
    pub fn grpc_service(&self) -> SearchServer<SearchGrpcService> {
        SearchServer::new(SearchGrpcService {
            index: self.index.clone(),
        })
    }

    /// Rebuild the entire search index from the database.
    async fn rebuild_index(&self) -> Result<()> {
        let handle_guard = self.handle.read().await;
        let handle = handle_guard
            .as_ref()
            .ok_or_else(|| eyre::eyre!("component not initialized"))?;

        info!("Rebuilding search index from database");

        // Create a new index
        let new_index = SearchIndex::new();

        // Load all history into the new index
        let db = handle.history_db().clone();
        let mut pager = db.all_paged(PAGE_SIZE, false, true);
        loop {
            match pager.next().await {
                Ok(Some(histories)) => {
                    info!(
                        "Loading {} history entries into search index",
                        histories.len()
                    );
                    new_index.add_histories(&histories);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("Failed to load history during rebuild: {}", e);
                    break;
                }
            }
        }

        info!(
            "Search index rebuild complete; {} unique commands",
            new_index.command_count()
        );

        // Replace the old index with the new one
        *self.index.write().await = new_index;
        Ok(())
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
        *self.handle.write().await = Some(handle.clone());

        // Spawn background task to load history into index
        let index = self.index.clone();
        let db = handle.history_db().clone();

        self.loader_handle = Some(tokio::spawn(async move {
            info!(
                "Loading history into search index; page size = {}",
                PAGE_SIZE
            );
            let mut pager = db.all_paged(PAGE_SIZE, false, true);
            loop {
                match pager.next().await {
                    Ok(Some(histories)) => {
                        info!(
                            "Loading {} history entries into search index",
                            histories.len()
                        );
                        index.read().await.add_histories(&histories);
                    }
                    Ok(None) => {
                        info!(
                            "Initial history load complete; {} unique commands indexed",
                            index.read().await.command_count()
                        );
                        // Build initial frecency map
                        index.read().await.rebuild_frecency().await;
                        info!("Initial frecency map built");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("Failed to load history: {}", e);
                        break;
                    }
                }
            }
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
                index_for_frecency.read().await.rebuild_frecency().await;
            }
        }));

        tracing::info!("search component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        match event {
            DaemonEvent::RecordsAdded(records) => {
                debug!(
                    count = records.len(),
                    "Processing added records for search index"
                );

                let handle_guard = self.handle.read().await;
                if let Some(handle) = handle_guard.as_ref() {
                    let histories: Vec<_> = handle
                        .history_db()
                        .query_history(
                            format!(
                                "select * from history where id in ({})",
                                records
                                    .iter()
                                    .map(|record| record.0.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )
                            .as_str(),
                        )
                        .await
                        .unwrap_or_default();

                    span!(Level::TRACE, "inject_records", count = histories.len())
                        .in_scope(async || {
                            self.index.read().await.add_histories(&histories);
                        })
                        .await;
                }
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
                if let Err(e) = self.rebuild_index().await {
                    tracing::error!("Failed to rebuild search index: {}", e);
                }
            }
            DaemonEvent::HistoryDeleted { ids } => {
                info!(
                    count = ids.len(),
                    "History deleted, rebuilding search index"
                );
                // For now, just rebuild the entire index. A more efficient implementation
                // would remove specific items from the index.
                if let Err(e) = self.rebuild_index().await {
                    tracing::error!("Failed to rebuild search index: {}", e);
                }
            }
            // Events we don't care about
            DaemonEvent::SyncCompleted { .. }
            | DaemonEvent::SyncFailed { .. }
            | DaemonEvent::ForceSync
            | DaemonEvent::SettingsReloaded
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

/// The gRPC service implementation.
pub struct SearchGrpcService {
    index: Arc<RwLock<SearchIndex>>,
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
        let index = self.index.clone();

        // Create output channel
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<SearchResponse, Status>>(128);

        // Spawn task to handle incoming requests and send responses
        tokio::spawn(async move {
            while let Some(req) = in_stream.message().await.transpose() {
                match req {
                    Ok(search_req) => {
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

                        // Build QueryContext from proto context
                        let query_context = proto_context
                            .map(|ctx| QueryContext {
                                cwd: Some(with_trailing_slash(&ctx.cwd)),
                                git_root: ctx.git_root.map(|s| with_trailing_slash(&s)),
                                hostname: Some(ctx.hostname),
                                session_id: Some(ctx.session_id),
                            })
                            .unwrap_or_default();

                        // Perform the search
                        let history_ids =
                            span!(Level::TRACE, "daemon_search_query", %query, query_id)
                                .in_scope(|| async {
                                    let index = index.read().await;
                                    index
                                        .search(&query, index_filter, &query_context, RESULTS_LIMIT)
                                        .await
                                })
                                .await;

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
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
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
            IndexFilterMode::Directory(with_trailing_slash(&ctx.cwd))
        }
        (FilterMode::Workspace, Some(ctx)) => {
            if let Some(ref git_root) = ctx.git_root {
                IndexFilterMode::Workspace(with_trailing_slash(git_root))
            } else {
                // Fall back to directory if no git root
                IndexFilterMode::Directory(with_trailing_slash(&ctx.cwd))
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

#[cfg(windows)]
pub fn with_trailing_slash(s: &str) -> String {
    if s.ends_with('\\') {
        s.to_string()
    } else {
        format!("{}\\", s)
    }
}

#[cfg(not(windows))]
pub fn with_trailing_slash(s: &str) -> String {
    if s.ends_with('/') {
        s.to_string()
    } else {
        format!("{}/", s)
    }
}
