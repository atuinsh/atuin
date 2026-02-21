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
    rx: broadcast::Receiver<DaemonEvent>,
}

impl SearchService {
    pub fn new(
        store: HistoryStore,
        history_db: HistoryDatabase,
        rx: broadcast::Receiver<DaemonEvent>,
    ) -> Self {
        Self {
            store,
            history_db,
            rx,
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
