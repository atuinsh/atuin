use atuin_client::{
    database::{Database, DbSearchMode, OptFilters},
    history::{History, all_user_author_filter},
    settings::Settings,
};
use atuin_daemon::client::{SearchClient, SearchParams};
use atuin_daemon::search::{normalize_diacritics, truncate_query};
use eyre::Result;
use tracing::{Level, debug, instrument, span};
use uuid::Uuid;

use super::{SearchEngine, SearchState};
use crate::command::client::daemon;

/// A lazily-initialized [`SearchClient`].
#[derive(Default)]
struct LazyClient(Option<SearchClient>);

impl LazyClient {
    /// Get the [`SearchClient`].
    #[instrument(skip_all, level = Level::TRACE, name = "get_daemon_client")]
    pub async fn get(&mut self, settings: &Settings) -> Result<&mut SearchClient> {
        // TODO: Ideally we would write this as follows, to avoid an `unwrap`:
        //
        //     Ok(match self.0.as_mut() {
        //         Some(client) => client,
        //         None => self.0.insert(self.connect(settings).await?),
        //     })
        //
        // However, Rust's borrow checker incorrectly rejects this code. This will be fixed by
        // Polonius; see https://rust-lang.github.io/rust-project-goals/2026/polonius.html.

        if self.0.is_none() {
            return Ok(self.0.insert(self.connect(settings).await?));
        }
        Ok(self.0.as_mut().unwrap())
    }

    async fn connect(&self, settings: &Settings) -> Result<SearchClient> {
        #[cfg(unix)]
        return SearchClient::new(settings.daemon.existing_socket_path().into_owned()).await;

        #[cfg(not(unix))]
        SearchClient::new(settings.daemon.tcp_port).await
    }

    /// Call a method on [`SearchClient`], autostarting the daemon and retrying if necessary.
    ///
    /// `f` should call the appropriate method on the provided [`SearchClient`].
    async fn try_with_autostart<F, R>(&mut self, settings: &Settings, mut f: F) -> Result<R>
    where
        F: AsyncFnMut(&mut SearchClient) -> Result<R>,
    {
        let err = match self.get(settings).await {
            Ok(client) => match f(client).await {
                Ok(result) => return Ok(result),
                Err(err) => err,
            },
            Err(err) => err,
        };

        if !(settings.daemon.autostart && daemon::should_retry_after_error(&err)) {
            return Err(err);
        }

        debug!("daemon not available, attempting auto-start");
        self.0 = None;
        if let Err(start_error) = daemon::ensure_daemon_running(settings).await {
            return Err(err.wrap_err(format!("failed to auto-start daemon: {start_error:#}")));
        }
        let client = self.get(settings).await?;
        f(client).await
    }
}

pub struct Search {
    client: LazyClient,
    settings: Settings,
    query_id: u64,
}

impl Search {
    pub fn new(settings: &Settings) -> Self {
        Search {
            client: LazyClient::default(),
            settings: settings.clone(),
            query_id: 0,
        }
    }

    fn next_query_id(&mut self) -> u64 {
        self.query_id += 1;
        self.query_id
    }

    /// Check if query contains regex pattern (r/.../)
    /// Nucleo doesn't support regex, so we fall back to database search
    fn contains_regex_pattern(query: &str) -> bool {
        query.starts_with("r/") || query.contains(" r/")
    }

    #[instrument(skip_all, level = Level::TRACE, name = "daemon_db_fallback")]
    async fn fallback_to_db_search(
        &self,
        state: &SearchState,
        db: &dyn Database,
    ) -> Result<Vec<History>> {
        let shells = state.shells.to_filter();
        Ok(db
            .search(
                DbSearchMode::FullText,
                state.filter_mode,
                &state.context,
                state.input.as_str(),
                OptFilters {
                    limit: Some(200),
                    authors: all_user_author_filter(),
                    shells: shells.as_filter(),
                    ..Default::default()
                },
            )
            .await?
            .into_iter()
            .collect())
    }

    #[instrument(skip_all, level = Level::TRACE, name = "hydrate_from_db", fields(count = ids.len()))]
    async fn hydrate_from_db(&self, db: &dyn Database, ids: &[String]) -> Result<Vec<History>> {
        let placeholders: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
        let sql_query = format!(
            "SELECT * FROM history WHERE id IN ({}) ORDER BY timestamp DESC",
            placeholders.join(",")
        );
        Ok(db.query_history(&sql_query).await?)
    }

    /// Tell the daemon to build the search index.
    pub async fn prepare_index(&mut self) -> Result<()> {
        self.client
            .try_with_autostart(&self.settings, async |client| {
                let shells = self.settings.search.shells.to_filter().to_vec_filter();
                client.prepare_index(shells).await
            })
            .await
    }
}

impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "daemon_search", fields(query = %state.input.as_str()))]
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        let query = state.input.as_str().to_string();

        // Fall back to database for regex queries (Nucleo doesn't support regex)
        if Self::contains_regex_pattern(&query) {
            debug!(query = %query, "[daemon-client] regex detected, falling back to db");
            return self.fallback_to_db_search(state, db).await;
        }

        let query_id = self.next_query_id();

        let span =
            span!(Level::TRACE, "daemon_search.req_resp", query = %query, query_id = query_id);

        let mut stream = self
            .client
            .try_with_autostart(&self.settings, async |client| {
                let search_params = SearchParams {
                    query: query.clone(),
                    query_id,
                    filter_mode: state.filter_mode,
                    context: Some(state.context.clone()),
                    shells: state.shells.to_filter().to_vec_filter(),
                };
                client.search(search_params).await
            })
            .await?;

        let mut ids = Vec::with_capacity(200);
        span!(Level::TRACE, "daemon_search.resp")
            .in_scope(async || -> Result<()> {
                while let Some(response) = stream.message().await? {
                    let span2 = span!(
                        Level::TRACE,
                        "daemon_search.resp.item",
                        query_id = response.query_id
                    );
                    let span2_guard = span2.enter();
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
                    drop(span2_guard);
                    drop(span2);
                }
                Ok(())
            })
            .await?;
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
        // Use fulltext highlighting for regex queries
        if Self::contains_regex_pattern(search_input) {
            return super::db::get_highlight_indices_fulltext(command, search_input);
        }

        // Mirror the daemon's query handling: truncate before frizbee sees
        // the query (a long enough atom panics Matcher::from_query) and
        // normalize diacritics so highlighting agrees with matching
        let search_input = normalize_diacritics(truncate_query(search_input));
        let matchable = normalize_diacritics(command);

        let config = frizbee::Config::default().casing(frizbee::CaseMatching::Smart);
        let mut matcher = frizbee::Matcher::from_query(&search_input, &config);
        let Some(result) = matcher.match_one_indices(&matchable, 0) else {
            return Vec::new();
        };

        // frizbee returns indices in reverse order, as byte offsets into
        // `matchable` (every byte of a matched multibyte char); the renderer
        // tests byte offsets into `command`, whose byte layout differs from
        // `matchable` wherever normalization shrank a char (é → e).
        // Normalization maps char to char, so hop matchable-byte → char
        // position → command-byte.
        let mut indices = result.indices;
        indices.sort_unstable();
        indices.dedup();
        if command.is_ascii() {
            indices.into_iter().map(|i| i as usize).collect()
        } else {
            let matchable_byte_to_char: std::collections::HashMap<usize, usize> = matchable
                .char_indices()
                .enumerate()
                .map(|(char_idx, (byte_idx, _))| (byte_idx, char_idx))
                .collect();
            let command_char_to_byte: Vec<usize> = command
                .char_indices()
                .map(|(byte_idx, _)| byte_idx)
                .collect();
            let mut bytes: Vec<usize> = indices
                .into_iter()
                .filter_map(|i| matchable_byte_to_char.get(&(i as usize)))
                .filter_map(|&char_idx| command_char_to_byte.get(char_idx).copied())
                .collect();
            bytes.dedup();
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: the daemon truncates queries before frizbee sees
    /// them, but highlighting used the raw input — a pasted query with an
    /// atom past frizbee's needle limit panicked in `Matcher::from_query`.
    #[test]
    fn long_query_does_not_panic_highlighting() {
        let engine = Search::new(&Settings::default());
        let long_query = "a".repeat(5000);
        let indices = engine.get_highlight_indices("echo hello", &long_query);
        assert!(indices.is_empty());
    }

    /// Highlighting matches accent-insensitively, like the daemon's index,
    /// and returns byte offsets into the original command — the renderer
    /// tests each display char's source byte against these ("echo déjà" is
    /// e0 c1 h2 o3 ␣4 d5 é6 j8 à9; é and à are two bytes each).
    #[test]
    fn accented_command_highlights_unaccented_query() {
        let engine = Search::new(&Settings::default());
        let indices = engine.get_highlight_indices("echo déjà", "deja");
        assert_eq!(indices, vec![5, 6, 8, 9]);
    }

    /// A multibyte char before the match must not shift the highlight:
    /// frizbee's offsets are into the normalized text ("emacs test"), which
    /// is one byte shorter than the command wherever é shrank to e.
    #[test]
    fn multibyte_char_before_match_does_not_shift_highlight() {
        let engine = Search::new(&Settings::default());
        let indices = engine.get_highlight_indices("émacs test", "test");
        assert_eq!(indices, vec![7, 8, 9, 10]);
    }

    /// Non-Latin text doesn't normalize, so matchable and command share a
    /// byte layout; offsets still land on the match ("日本 git" is 日0 本3
    /// ␣6 g7 i8 t9).
    #[test]
    fn cjk_prefix_highlights_at_correct_bytes() {
        let engine = Search::new(&Settings::default());
        let indices = engine.get_highlight_indices("日本 git", "git");
        assert_eq!(indices, vec![7, 8, 9]);
    }
}
