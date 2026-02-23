use async_trait::async_trait;
use atuin_client::{database::Database, history::History, settings::Settings};
use atuin_daemon::client::SearchClient;
use eyre::Result;
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

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
}

#[async_trait]
impl SearchEngine for Search {
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        let query = state.input.as_str().to_string();
        let query_id = self.next_query_id();

        let client = self.get_client().await?;
        let mut stream = client.search(query, query_id).await?;

        // Get the first response (we expect only one response per query in this model)
        let mut ids = Vec::new();
        if let Some(response) = stream.message().await? {
            // Only process if the query_id matches (prevents stale responses)
            if response.query_id == query_id {
                ids = response.ids;
            }
        }

        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate from local database
        let placeholders: Vec<String> = ids.iter().map(|id| format!("'{}'", id)).collect();
        let query = format!(
            "SELECT * FROM history WHERE id IN ({}) ORDER BY timestamp DESC",
            placeholders.join(",")
        );
        let results = db.query_history(&query).await?;

        // Reorder results to match the order from the daemon (which is ranked by relevance)
        let mut ordered_results = Vec::with_capacity(results.len());
        for id in &ids {
            if let Some(history) = results.iter().find(|h| h.id.0 == *id) {
                ordered_results.push(history.clone());
            }
        }

        Ok(ordered_results)
    }

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
