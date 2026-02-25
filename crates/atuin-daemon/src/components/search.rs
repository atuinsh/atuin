//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library.

use std::{collections::HashSet, pin::Pin, sync::Arc};

use atuin_client::{database::Database, history::History};
use eyre::Result;
use nucleo::{Injector, Nucleo};
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, debug, info, instrument, span};
use uuid::Uuid;

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    search::{
        FilterMode, SearchRequest, SearchResponse,
        search_server::{Search as SearchSvc, SearchServer},
    },
};

const PAGE_SIZE: usize = 1000;
const RESULTS_LIMIT: u32 = 200;

/// Search component - provides fuzzy search over command history.
///
/// This component:
/// - Maintains an in-memory search index using Nucleo
/// - Loads history from the database on startup
/// - Updates the index when history events occur
/// - Provides the Search gRPC service
pub struct SearchComponent {
    inner: Arc<SearchComponentInner>,
    loader_handle: Option<tokio::task::JoinHandle<()>>,
}

struct SearchComponentInner {
    nucleo: RwLock<Nucleo<History>>,
    injector: Injector<History>,
    handle: RwLock<Option<DaemonHandle>>,
}

impl SearchComponent {
    /// Create a new search component.
    pub fn new() -> Self {
        let nucleo_config = nucleo::Config::DEFAULT.clone();
        let nucleo = Nucleo::<History>::new(nucleo_config, Arc::new(|| {}), None, 4);
        let injector = nucleo.injector();

        Self {
            inner: Arc::new(SearchComponentInner {
                nucleo: RwLock::new(nucleo),
                injector,
                handle: RwLock::new(None),
            }),
            loader_handle: None,
        }
    }

    /// Get the gRPC service for this component.
    pub fn grpc_service(&self) -> SearchServer<SearchGrpcService> {
        SearchServer::new(SearchGrpcService {
            inner: self.inner.clone(),
        })
    }

    /// Rebuild the entire search index from the database.
    async fn rebuild_index(&self) -> Result<()> {
        let handle_guard = self.inner.handle.read().await;
        let handle = handle_guard
            .as_ref()
            .ok_or_else(|| eyre::eyre!("component not initialized"))?;

        info!("Rebuilding search index from database");

        // Clear the current index by creating a new Nucleo instance
        let nucleo_config = nucleo::Config::DEFAULT.clone();
        let new_nucleo = Nucleo::<History>::new(nucleo_config, Arc::new(|| {}), None, 4);

        // Get the new injector before we replace nucleo
        let injector = new_nucleo.injector();

        // Replace the nucleo instance
        *self.inner.nucleo.write().await = new_nucleo;

        // Load all history into the new index
        let db = handle.history_db().clone();
        let mut pager = db.all_paged(PAGE_SIZE, false, true);
        loop {
            match pager.next().await {
                Ok(Some(history)) => {
                    info!(
                        "Loading {} history entries into search index",
                        history.len()
                    );
                    push_history(&history, &injector);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("Failed to load history during rebuild: {}", e);
                    break;
                }
            }
        }

        info!("Search index rebuild complete");
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
        *self.inner.handle.write().await = Some(handle.clone());

        // Spawn background task to load history into index
        let injector = self.inner.injector.clone();
        let db = handle.history_db().clone();

        self.loader_handle = Some(tokio::spawn(async move {
            info!(
                "Loading history into search index; page size = {}",
                PAGE_SIZE
            );
            let mut pager = db.all_paged(PAGE_SIZE, false, true);
            loop {
                match pager.next().await {
                    Ok(Some(history)) => {
                        info!(
                            "Loading {} history entries into search index",
                            history.len()
                        );
                        push_history(&history, &injector);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::error!("Failed to load history: {}", e);
                        break;
                    }
                }
            }
            info!("Initial history load complete");
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

                let handle_guard = self.inner.handle.read().await;
                if let Some(handle) = handle_guard.as_ref() {
                    let histories: Vec<History> = handle
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

                    span!(Level::TRACE, "inject_records", count = histories.len()).in_scope(|| {
                        push_history(&histories, &self.inner.injector);
                    });
                }
            }
            DaemonEvent::HistoryStarted(history) => {
                debug!(id = %history.id, command = %history.command, "History started (no index action)");
            }
            DaemonEvent::HistoryEnded(history) => {
                span!(Level::TRACE, "inject_history_ended").in_scope(|| {
                    push_history(&[history.clone()], &self.inner.injector);
                });
            }
            DaemonEvent::HistoryPruned => {
                info!("History pruned, rebuilding search index");
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
                // would remove specific items from the Nucleo index.
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
        tracing::info!("search component stopped");
        Ok(())
    }
}

/// Push history items into the Nucleo search index.
fn push_history(history: &[History], injector: &Injector<History>) {
    for h in history {
        injector.push(h.clone(), |history, columns| {
            columns[0] = history.command.clone().into();
            columns[1] = with_trailing_slash(&history.cwd).into();
            columns[2] = history.hostname.clone().into();
            columns[3] = history.session.clone().into();
        });
    }
}

#[cfg(windows)]
fn with_trailing_slash(s: &str) -> String {
    if s.ends_with('\\') {
        s.to_string()
    } else {
        format!("{}\\", s)
    }
}

#[cfg(not(windows))]
fn with_trailing_slash(s: &str) -> String {
    if s.ends_with('/') {
        s.to_string()
    } else {
        format!("{}/", s)
    }
}

/// The gRPC service implementation.
pub struct SearchGrpcService {
    inner: Arc<SearchComponentInner>,
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
        let inner = self.inner.clone();

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
                        let context = search_req.context;

                        debug!(
                            "search request: query = {}, query_id = {}, filter_mode = {}, context = {:?}",
                            query,
                            query_id,
                            filter_mode.as_str_name(),
                            context
                        );

                        let mut nucleo = inner.nucleo.write().await;
                        nucleo.pattern = nucleo::pattern::MultiPattern::new(6);

                        // Update the search pattern and tick until complete
                        let ids = span!(Level::TRACE, "daemon_search_query", %query, query_id)
                            .in_scope(async || {
                                span!(Level::TRACE, "nucleo_match", %query, query_id)
                                    .in_scope(async || {
                                        nucleo.pattern.reparse(
                                            0,
                                            &query,
                                            nucleo::pattern::CaseMatching::Smart,
                                            nucleo::pattern::Normalization::Smart,
                                            false,
                                        );

                                        match (filter_mode, context) {
                                            (FilterMode::Global, _) => {}
                                            (FilterMode::Host, Some(context)) => {
                                                nucleo.pattern.reparse(
                                                    2,
                                                    format!("^{}$", context.hostname).as_str(),
                                                    nucleo::pattern::CaseMatching::Ignore,
                                                    nucleo::pattern::Normalization::Never,
                                                    false,
                                                );
                                            }
                                            (FilterMode::Session, Some(context)) => {
                                                nucleo.pattern.reparse(
                                                    3,
                                                    format!("^{}$", context.session_id).as_str(),
                                                    nucleo::pattern::CaseMatching::Ignore,
                                                    nucleo::pattern::Normalization::Never,
                                                    false,
                                                );
                                            }
                                            (FilterMode::Directory, Some(context)) => {
                                                nucleo.pattern.reparse(
                                                    1,
                                                    format!(
                                                        "^{}$",
                                                        with_trailing_slash(&context.cwd)
                                                    )
                                                    .as_str(),
                                                    nucleo::pattern::CaseMatching::Ignore,
                                                    nucleo::pattern::Normalization::Never,
                                                    false,
                                                );
                                            }
                                            (FilterMode::Workspace, Some(context)) => {
                                                if let Some(git_root) = &context.git_root {
                                                    nucleo.pattern.reparse(
                                                        1,
                                                        format!(
                                                            "^{}",
                                                            with_trailing_slash(git_root)
                                                        )
                                                        .as_str(),
                                                        nucleo::pattern::CaseMatching::Ignore,
                                                        nucleo::pattern::Normalization::Never,
                                                        false,
                                                    );
                                                }
                                            }
                                            (FilterMode::SessionPreload, Some(context)) => {
                                                nucleo.pattern.reparse(
                                                    3,
                                                    format!("^{}", context.session_id).as_str(),
                                                    nucleo::pattern::CaseMatching::Ignore,
                                                    nucleo::pattern::Normalization::Never,
                                                    false,
                                                );
                                            }
                                            _ => {}
                                        }

                                        // Tick until processing is complete
                                        span!(Level::TRACE, "tick_until_complete", %query, query_id)
                                            .in_scope(|| while nucleo.tick(10).running {});

                                        // Get the snapshot and collect IDs
                                        let snapshot = nucleo.snapshot();
                                        let matched_count =
                                            snapshot.matched_item_count().min(RESULTS_LIMIT);

                                        span!(Level::TRACE, "dedup_and_send_results", %query, query_id).in_scope(async || {
                                            let mut seen: HashSet<String> = HashSet::new();
                                            let mut ids: Vec<Vec<u8>> = Vec::with_capacity(matched_count as usize);

                                            for item in snapshot.matched_items(..matched_count) {
                                                if seen.contains(&item.data.command) {
                                                    continue;
                                                }

                                                seen.insert(item.data.command.clone());
                                                let uuid = item.data.id.0.clone();
                                                let uuid = Uuid::parse_str(&uuid).expect("invalid uuid");
                                                let bytes = uuid.as_bytes().to_vec();
                                                ids.push(bytes);
                                            }

                                            ids
                                        })
                                        .await
                                    })
                                    .await
                            })
                            .await;

                        drop(nucleo);

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
