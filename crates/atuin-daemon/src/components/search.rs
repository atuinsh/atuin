//! Search component.
//!
//! Provides fuzzy search over command history using the Nucleo search library
//! with frecency-based ranking and dynamic filtering.

use std::pin::Pin;
use std::sync::Arc;

use atuin_common::filter::OrFilter;
use atuin_common::path::DisplayRichExt;
use eyre::Result;
use futures::StreamExt;
use tokio::sync::{RwLock, RwLockReadGuard};
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Instrument as _, Level, debug, error, info, instrument, span, trace};

use crate::daemon::{Component, DaemonHandle};
use crate::events::DaemonEvent;
use crate::output_capture::OutputStore;
use crate::search::search_server::{Search as SearchSvc, SearchServer};
use crate::search::{
    FilterMode, IndexFilterMode, OutputSearchLine, OutputSearchMatch, PrepareIndexRequest,
    PrepareIndexResponse, RebuildIndexRequest, RebuildIndexResponse, SearchCommandOutputRequest,
    SearchIndex, SearchRequest, SearchResponse,
};

const RESULTS_LIMIT: usize = 200;
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
    handle: Option<DaemonHandle>,
    loader_handle: Option<tokio::task::JoinHandle<()>>,
    frecency_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SearchComponent {
    /// Create a new search component.
    #[must_use]
    pub fn new() -> Self {
        Self {
            index: Arc::new(RwLock::new(SearchIndex::default())),
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
    pub fn index(&self) -> Arc<RwLock<SearchIndex>> {
        self.index.clone()
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn rebuild_frecency(handle: &DaemonHandle, index: &RwLock<SearchIndex>) {
        let settings = handle.settings().await;
        index.read().await.rebuild_frecency(&settings.search);
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

        // Configure the initial filter before exposing the service. A later shell-filter reload
        // is serialized with the initial load and chooses its own filter when it acquires the lock.
        let shells = handle.settings().await.search.shells.to_filter().to_vec_filter();
        self.index.write().await.shells = shells;

        // Spawn background task to load history into index
        let initial = SearchIndex::initial_loader(self.index.clone()).await;
        let handle_for_loader = handle.clone();

        self.loader_handle = Some(tokio::spawn(async move {
            let search = handle_for_loader.settings().await.search.clone();
            let _ =
                initial.load(handle_for_loader.history_db(), &search).await.inspect_err(|err| {
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
            DaemonEvent::ShutdownRequested => {}
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
    index: Arc<RwLock<SearchIndex>>,
    output_store: Arc<OutputStore>,
    handle: DaemonHandle,
}

impl SearchGrpcService {
    async fn maybe_rebuild_index(
        &self,
        shells: OrFilter<Vec<String>>,
    ) -> Result<RwLockReadGuard<'_, SearchIndex>, ()> {
        let index = self.index.read().await;
        if index.shells == shells {
            return Ok(index);
        }
        drop(index);
        let search = self.handle.settings().await.search.clone();
        SearchIndex::reload(&self.index, Some(shells), self.handle.history_db(), &search)
            .await
            .map_err(|e| error!("Failed to rebuild search index: {e}"))
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
                let Ok(index) = this.maybe_rebuild_index(shells).await else {
                    let _ = tx.send(Err(Status::internal("failed to build index"))).await;
                    break;
                };

                // Perform the search
                let history_ids: Vec<Vec<u8>> =
                    span!(Level::TRACE, "daemon_search_query", %query, query_id).in_scope(|| {
                        index
                            .search(&query, &index_filter, RESULTS_LIMIT)
                            .map(|id| id.into_bytes().to_vec())
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
        drop(
            self.maybe_rebuild_index(shells)
                .await
                .map_err(|()| Status::internal("failed to build index"))?,
        );
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
                let search = handle.settings().await.search.clone();
                let command_count = SearchIndex::reload(&index, None, handle.history_db(), &search)
                    .await?
                    .command_count();
                info!("Search index rebuild complete; {command_count} unique commands");
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
