use std::{pin::Pin, sync::Arc};

use atuin_client::{
    database::{Database, Sqlite as HistoryDatabase},
    history::{History, store::HistoryStore},
};
use nucleo::Nucleo;
use time::OffsetDateTime;
use tokio::sync::{RwLock, broadcast};
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;

use crate::{
    events::DaemonEvent,
    search::{SearchRequest, SearchResponse, search_server::Search as SearchSvc},
};

pub struct SearchService {
    store: HistoryStore,
    history_db: HistoryDatabase,
    tx: broadcast::Sender<DaemonEvent>,
    nucleo: Arc<RwLock<Nucleo<History>>>,
}

impl SearchService {
    pub fn new(
        store: HistoryStore,
        history_db: HistoryDatabase,
        tx: broadcast::Sender<DaemonEvent>,
    ) -> Self {
        let mut rx = tx.subscribe();
        let nucleo_config = nucleo::Config::DEFAULT.clone();

        let nucleo = Nucleo::<History>::new(nucleo_config, Arc::new(|| {}), None, 2);

        let injector = nucleo.injector();
        let injector_clone = injector.clone();
        let nucleo = Arc::new(RwLock::new(nucleo));

        let history_db_clone = history_db.clone();
        tokio::spawn(async move {
            loop {
                let event = rx.recv().await.unwrap();
                match event {
                    DaemonEvent::RecordsAdded(records) => {
                        println!("Added {} records", records.len());
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
                        for history in histories {
                            injector_clone.push(history, |history, columns| {
                                columns[0] = history.command.clone().into();
                                columns[1] = history.cwd.to_string().into();
                            });
                        }
                    }
                    DaemonEvent::HistoryStarted(history) => {
                        println!("history started: {:?}", history);
                    }
                    DaemonEvent::HistoryEnded(history) => {
                        injector_clone.push(history, |history, columns| {
                            columns[0] = history.command.clone().into();
                            columns[1] = history.cwd.to_string().into();
                        });
                    }
                }
            }
        });

        // Load history from the database asynchronously
        let db = history_db.clone();
        tokio::spawn(async move {
            // Load recent history (last 10000 entries)
            match db.before(OffsetDateTime::now_utc(), 10000).await {
                Ok(history) => {
                    info!(
                        "Loading {} history entries into search index",
                        history.len()
                    );
                    for h in history {
                        injector.push(h, |history, columns| {
                            columns[0] = history.command.clone().into();
                            columns[1] = history.cwd.to_string().into();
                        });
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to load history: {}", e);
                }
            }
        });

        Self {
            store,
            history_db,
            tx,
            nucleo,
        }
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
                        let mut nucleo = nucleo.write().await;

                        // Update the search pattern
                        nucleo.pattern.reparse(
                            0,
                            &query,
                            nucleo::pattern::CaseMatching::Smart,
                            nucleo::pattern::Normalization::Smart,
                            false,
                        );

                        // Tick until processing is complete
                        while nucleo.tick(10).running {}

                        // Get the snapshot and collect IDs as strings
                        let snapshot = nucleo.snapshot();
                        let ids: Vec<String> = snapshot
                            .matched_items(..snapshot.matched_item_count().min(200))
                            .map(|item| item.data.id.0.clone())
                            .collect();

                        if tx.send(Ok(SearchResponse { ids, query_id })).await.is_err() {
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
