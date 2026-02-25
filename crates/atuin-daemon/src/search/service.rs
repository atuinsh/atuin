use std::{collections::HashSet, pin::Pin, sync::Arc};

use atuin_client::{
    database::{Database, Sqlite as HistoryDatabase},
    history::{History, store::HistoryStore},
};
use nucleo::{Injector, Nucleo};
use tokio::sync::{RwLock, broadcast};
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, debug, info, instrument, span};
use uuid::Uuid;

use super::{FilterMode, SearchRequest, SearchResponse, search_server::Search as SearchSvc};
use crate::events::DaemonEvent;

const PAGE_SIZE: usize = 1000;
const RESULTS_LIMIT: u32 = 200;

pub struct SearchService {
    nucleo: Arc<RwLock<Nucleo<History>>>,
}

impl SearchService {
    pub fn new(
        _store: HistoryStore,
        history_db: HistoryDatabase,
        tx: broadcast::Sender<DaemonEvent>,
    ) -> Self {
        let mut rx = tx.subscribe();
        let nucleo_config = nucleo::Config::DEFAULT.clone();

        let nucleo = Nucleo::<History>::new(nucleo_config, Arc::new(|| {}), None, 4);

        let injector = nucleo.injector();
        let injector_clone = injector.clone();
        let nucleo = Arc::new(RwLock::new(nucleo));

        let history_db_clone = history_db.clone();
        tokio::spawn(async move {
            loop {
                let event = rx.recv().await.unwrap();
                match event {
                    DaemonEvent::RecordsAdded(records) => {
                        debug!(count = records.len(), "Processing added records");
                        let histories: Vec<History> = history_db_clone
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
                            .unwrap();
                        span!(Level::TRACE, "inject_records", count = histories.len()).in_scope(
                            || {
                                push_history(&histories, &injector_clone);
                            },
                        );
                    }
                    DaemonEvent::HistoryStarted(history) => {
                        debug!(id = %history.id, command = %history.command, "History started");
                    }
                    DaemonEvent::HistoryEnded(history) => {
                        span!(Level::TRACE, "inject_history_ended").in_scope(|| {
                            push_history(&[history], &injector_clone);
                        });
                    }
                    // TODO: Handle these events properly when SearchComponent is implemented
                    DaemonEvent::HistoryPruned | DaemonEvent::HistoryDeleted { .. } => {
                        // These require rebuilding the index - will be handled in SearchComponent
                        debug!("received history change event, index rebuild needed");
                    }
                    // Events we don't care about in search
                    DaemonEvent::SyncCompleted { .. }
                    | DaemonEvent::SyncFailed { .. }
                    | DaemonEvent::ForceSync
                    | DaemonEvent::SettingsReloaded
                    | DaemonEvent::ShutdownRequested => {}
                }
            }
        });

        // Load history from the database asynchronously
        let db = history_db.clone();
        tokio::spawn(async move {
            // Load all history into the search index,
            // deduplicated and without deleted entries.
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
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        tracing::error!("Failed to load history: {}", e);
                        break;
                    }
                }
            }
        });

        Self { nucleo }
    }
}

fn push_history(history: &[History], injector: &Injector<History>) {
    for history in history {
        injector.push(history.clone(), |history, columns| {
            columns[0] = history.command.clone().into();
            columns[1] = with_trailing_slash(&history.cwd).into();
            columns[2] = history.hostname.clone().into();
            columns[3] = history.session.clone().into();
        });
    }
}

#[tonic::async_trait()]
impl SearchSvc for SearchService {
    // Output stream type - what we send back to the client
    type SearchStream = Pin<Box<dyn Stream<Item = Result<SearchResponse, Status>> + Send>>;

    /// Search for a query and return a stream of search responses
    ///
    /// The client will send a search request with a query and a query ID.
    /// The server will respond with a search response with a list of history IDs.
    ///
    /// The query ID allows the server to stream back new items as they're added to the index,
    /// but this is not implemented yet.
    #[instrument(skip_all, level = Level::TRACE, name = "search_rpc")]
    async fn search(
        &self,
        request: Request<Streaming<SearchRequest>>,
    ) -> Result<Response<Self::SearchStream>, Status> {
        let mut in_stream = request.into_inner();
        let nucleo = self.nucleo.clone();

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

                        let mut nucleo = nucleo.write().await;
                        nucleo.pattern = nucleo::pattern::MultiPattern::new(6);

                        // Update the search pattern and tick until complete (sync work)
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
                                            (FilterMode::Global, _) => { /* nothing to do */ }
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

                                        // Get the snapshot and collect IDs as strings
                                        let snapshot = nucleo.snapshot();
                                        let matched_count =
                                            snapshot.matched_item_count().min(RESULTS_LIMIT);

                                        span!(Level::TRACE, "dedup_and_send_results", %query, query_id).in_scope(async || {
                                            // History entries are deduplicated by command + cwd + hostname + session;
                                            // this is so filter modes work correctly. To deduplicate the actual commands,
                                            // we need to keep track of which ones we've already seen.
                                            let mut seen: HashSet<String> = HashSet::new();
                                            let mut ids: Vec<Vec<u8>> = Vec::with_capacity(matched_count as usize);

                                            for item in snapshot.matched_items(..matched_count) {
                                                if seen.contains(&item.data.command) {
                                                    continue;
                                                }

                                                seen.insert(item.data.command.clone());
                                                let uuid = item.data.id.0.clone();
                                                let uuid = Uuid::parse_str(&uuid).expect("invalid uuid"); // todo
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
