use std::sync::Arc;

use async_trait::async_trait;
use atuin_client::{
    database::Database,
    history::{History, HistoryId},
    settings::Settings,
};
use atuin_daemon::client::SearchClient;
use eyre::{Result, eyre};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::{Level, debug, instrument, span};
use uuid::Uuid;

use super::{SearchEngine, SearchState};

pub struct Search {
    client: Option<SearchClient>,
    query_id: u64,
    socket_path: String,
    #[cfg(not(unix))]
    tcp_port: u64,
}

impl Search {
    pub fn new(settings: &Settings) -> Self {
        Search {
            client: None,
            query_id: 0,
            socket_path: settings.daemon.socket_path.clone(),
            #[cfg(not(unix))]
            tcp_port: settings.daemon.tcp_port,
        }
    }

    #[instrument(skip_all, level = Level::TRACE, name = "get_daemon_client")]
    async fn get_client(&mut self) -> Result<&mut SearchClient> {
        if self.client.is_none() {
            #[cfg(unix)]
            let client = SearchClient::new(self.socket_path.clone()).await?;

            #[cfg(not(unix))]
            let client = SearchClient::new(self.tcp_port).await?;

            self.client = Some(client);
        }
        Ok(self.client.as_mut().unwrap())
    }

    fn next_query_id(&mut self) -> u64 {
        self.query_id += 1;
        self.query_id
    }

    #[instrument(skip_all, level = Level::TRACE, name = "hydrate_from_db", fields(count = ids.len()))]
    async fn hydrate_from_db(&self, db: &mut dyn Database, ids: &[String]) -> Result<Vec<History>> {
        let placeholders: Vec<String> = ids.iter().map(|id| format!("'{}'", id)).collect();
        let sql_query = format!(
            "SELECT * FROM history WHERE id IN ({}) ORDER BY timestamp DESC",
            placeholders.join(",")
        );
        Ok(db.query_history(&sql_query).await?)
    }
}

#[async_trait]
impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "daemon_search", fields(query = %state.input.as_str()))]
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        let query = state.input.as_str().to_string();
        let query_id = self.next_query_id();

        let span =
            span!(Level::TRACE, "daemon_search.req_resp", query = %query, query_id = query_id);

        let client = self.get_client().await?;

        let _span = span.enter();
        let mut stream = client
            .search(
                query.clone(),
                query_id,
                state.filter_mode,
                Some(state.context.clone()),
            )
            .await?;

        let mut ids = Vec::with_capacity(200);
        span!(Level::TRACE, "daemon_search.resp")
            .in_scope(async || {
                while let Ok(Some(response)) = stream.message().await {
                    let span2 = span!(
                        Level::TRACE,
                        "daemon_search.resp.item",
                        query_id = response.query_id
                    );
                    let _span2 = span2.enter();
                    // Only process if the query_id matches (prevents stale responses)
                    if response.query_id == query_id {
                        let uuids = response
                            .ids
                            .iter()
                            .map(|id| {
                                let bytes: [u8; 16] =
                                    id.as_slice().try_into().expect("id should be 16 bytes");
                                Uuid::from_bytes(bytes).as_simple().to_string()
                            })
                            .collect::<Vec<_>>();
                        ids.extend(uuids);
                    }
                    drop(_span2);
                    drop(span2);
                }
            })
            .await;
        drop(_span);
        drop(span);

        if ids.is_empty() {
            debug!(query = %query, results = 0, "[daemon-client] empty results");
            return Ok(Vec::new());
        }

        // // Hydrate from local database
        let results = self.hydrate_from_db(db, &ids).await?;

        // // Reorder results to match the order from the daemon (which is ranked by relevance)
        let ordered_results = span!(Level::TRACE, "reorder_results").in_scope(|| {
            let mut ordered_results = Vec::with_capacity(results.len());
            for id in &ids {
                if let Some(history) = results.iter().find(|h| h.id.0 == *id) {
                    ordered_results.push(history.clone());
                }
            }
            ordered_results
        });

        debug!(
            query = %query,
            results = results.len(),
            "[daemon-client]"
        );

        Ok(ordered_results)
    }

    #[instrument(skip_all, level = Level::TRACE, name = "daemon_highlight")]
    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(search_input, CaseMatching::Smart, Normalization::Smart);

        let mut indices: Vec<u32> = Vec::new();
        let mut haystack_buf = Vec::new();

        let haystack = Utf32Str::new(command, &mut haystack_buf);
        pattern.indices(haystack, &mut matcher, &mut indices);

        // Convert u32 indices to usize
        indices.into_iter().map(|i| i as usize).collect()
    }
}

struct Hydrator {
    tasks: Vec<JoinHandle<Result<Vec<History>>>>,
    results: Arc<RwLock<Vec<History>>>,
}

impl Hydrator {
    pub fn new() -> Self {
        Hydrator {
            tasks: Vec::new(),
            results: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn hydrate(&mut self, db: Box<dyn Database + 'static>, ids: &[String]) {
        let placeholders: Vec<String> = ids.iter().map(|id| format!("'{}'", id)).collect();

        let task = tokio::spawn(async move {
            let sql_query = format!(
                "SELECT * FROM history WHERE id IN ({}) ORDER BY timestamp DESC",
                placeholders.join(",")
            );
            db.query_history(&sql_query).await.map_err(|e| eyre!(e))
        });

        self.tasks.push(task);
    }

    pub async fn results(&mut self) -> Vec<History> {
        for task in self.tasks.drain(..) {
            if let Ok(results) = task.await.expect("failed to join task") {
                self.results.write().await.extend(results.into_iter());
            }
        }
        self.results.read().await.clone()
    }
}
