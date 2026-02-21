use atuin_client::{database::Sqlite as HistoryDatabase, history::store::HistoryStore};
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};

use crate::{
    events::DaemonEvent,
    search::{SearchRequest, SearchResponse, search_server::Search as SearchSvc},
};

pub struct SearchService {
    store: HistoryStore,
    history_db: HistoryDatabase,
    tx: broadcast::Sender<DaemonEvent>,
}

impl SearchService {
    pub fn new(
        store: HistoryStore,
        history_db: HistoryDatabase,
        tx: broadcast::Sender<DaemonEvent>,
    ) -> Self {
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            loop {
                let event = rx.recv().await.unwrap();
                match event {
                    DaemonEvent::RecordsAdded(records) => {
                        println!("records added: {:?}", records);
                    }
                    DaemonEvent::HistoryStarted(history) => {
                        println!("history started: {:?}", history);
                    }
                    DaemonEvent::HistoryEnded(history) => {
                        println!("history ended: {:?}", history);
                    }
                }
            }
        });

        Self {
            store,
            history_db,
            tx,
        }
    }
}

#[tonic::async_trait()]
impl SearchSvc for SearchService {
    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        Ok(Response::new(SearchResponse {
            result: "Hello, world!".to_string(),
        }))
    }
}
