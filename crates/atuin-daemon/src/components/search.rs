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
        let handle_for_loader = handle.clone();

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
                        // Build initial frecency map with current settings
                        let settings = handle_for_loader.settings().await;
                        index.read().await.rebuild_frecency(&settings.search).await;
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
        let handle_for_frecency = handle.clone();
        self.frecency_handle = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                FRECENCY_REFRESH_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;
                trace!("Refreshing frecency map");
                let settings = handle_for_frecency.settings().await;
                index_for_frecency
                    .read()
                    .await
                    .rebuild_frecency(&settings.search)
                    .await;
            }
        }));

        tracing::info!("search component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        match event {
            DaemonEvent::HistorySynced(ids) => {
                debug!(count = ids.len(), "Indexing synced history entries");

                let handle_guard = self.handle.read().await;
                let Some(handle) = handle_guard.as_ref() else {
                    return Ok(());
                };

                // Propagates rather than unwrap_or_default: a read failure here means the
                // index is silently missing entries, which is exactly #3627's symptom.
                let histories = handle.history_db().load_bulk(ids).await?;

                span!(
                    Level::TRACE,
                    "inject_synced_history",
                    count = histories.len()
                )
                .in_scope(async || {
                    self.index.read().await.add_histories(&histories);
                })
                .await;
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
            DaemonEvent::SettingsReloaded => {
                info!("Settings reloaded, rebuilding frecency map with new multipliers");
                let handle_guard = self.handle.read().await;
                if let Some(handle) = handle_guard.as_ref() {
                    let settings = handle.settings().await;
                    self.index
                        .read()
                        .await
                        .rebuild_frecency(&settings.search)
                        .await;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use atuin_client::database::{Database, Sqlite};
    use atuin_client::history::{History, HistoryId};
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{Settings, init_meta_config_for_testing};
    use tempfile::TempDir;
    use time::OffsetDateTime;

    use super::*;
    use crate::Daemon;

    async fn test_handle(tmp: &TempDir) -> (DaemonHandle, Sqlite) {
        let db_path = tmp.path().join("history.db");
        let record_path = tmp.path().join("records.db");
        let key_path = tmp.path().join("key");
        let meta_path = tmp.path().join("meta.db");

        init_meta_config_for_testing(meta_path.to_str().unwrap(), 5.0);

        let settings: Settings = Settings::builder()
            .expect("could not build settings builder")
            .set_override("db_path", db_path.to_str().unwrap())
            .expect("failed to set db_path")
            .set_override("record_store_path", record_path.to_str().unwrap())
            .expect("failed to set record_store_path")
            .set_override("key_path", key_path.to_str().unwrap())
            .expect("failed to set key_path")
            .set_override("meta.db_path", meta_path.to_str().unwrap())
            .expect("failed to set meta.db_path")
            .build()
            .expect("could not build settings")
            .try_deserialize()
            .expect("could not deserialize settings");

        let history_db = Sqlite::new(&db_path, 5.0).await.unwrap();
        let store = SqliteStore::new(&record_path, 5.0).await.unwrap();

        let daemon = Daemon::builder(settings)
            .store(store)
            .history_db(history_db.clone())
            .build()
            .await
            .unwrap();

        (daemon.handle(), history_db)
    }

    async fn save_history(db: &Sqlite, cmd: &str) -> History {
        let mut captured: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(cmd)
            .cwd("/home/ellie")
            .build()
            .into();

        captured.exit = 0;
        captured.duration = 1;
        // Deliberately not overriding `session` (unlike the copy-pasted helper this was
        // based on in atuin-client's database.rs tests): the search index only counts a
        // history entry if its session parses as a UUID (see
        // SearchIndex::add_history -> CommandData::new -> parse_uuid_bytes), and
        // History::new() already gives `captured` a valid random session. Overwriting it
        // with a non-UUID placeholder silently drops the entry from the index and makes
        // this test's command_count assertions fail for reasons unrelated to the handler
        // under test.
        captured.hostname = "booop".to_string();

        db.save(&captured).await.unwrap();
        captured
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_synced_indexes_the_named_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let (handle, db) = test_handle(&tmp).await;

        let alpha = save_history(&db, "echo alpha").await;
        let bravo = save_history(&db, "echo bravo").await;
        let _not_in_the_event = save_history(&db, "echo charlie").await;

        let mut component = SearchComponent::new();
        // Injected rather than start()ed: start() spawns a loader that pages the whole
        // db into the index, which would index charlie too and hide a broken handler.
        *component.handle.write().await = Some(handle);

        let ids: Arc<[HistoryId]> = vec![alpha.id.clone(), bravo.id.clone()].into();
        component
            .handle_event(&DaemonEvent::HistorySynced(ids))
            .await
            .unwrap();

        assert_eq!(component.index.read().await.command_count(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_synced_with_no_ids_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let (handle, db) = test_handle(&tmp).await;
        save_history(&db, "echo alpha").await;

        let mut component = SearchComponent::new();
        *component.handle.write().await = Some(handle);

        let ids: Arc<[HistoryId]> = Vec::new().into();
        component
            .handle_event(&DaemonEvent::HistorySynced(ids))
            .await
            .unwrap();

        assert_eq!(component.index.read().await.command_count(), 0);
    }
}
