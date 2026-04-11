use super::{SearchEngine, SearchState, search_db};
use async_trait::async_trait;
use atuin_client::{
    database::Database,
    database::{QueryToken, QueryTokenizer},
    history::History,
    settings::SearchMode,
};
use eyre::Result;
use norm::Metric;
use norm::fzf::{FzfParser, FzfV2};
use std::ops::Range;
use tracing::{Level, instrument};

pub struct Search(pub SearchMode);

#[async_trait]
impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "db_search", fields(mode = ?self.0, query = %state.input.as_str()))]
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        search_db(state, db, self.0, state.input.as_str())
            .await
            // ignore errors as it may be caused by incomplete regex
            .map_or_else(|_| Ok(Vec::new()), Ok)
    }

    #[instrument(skip_all, level = Level::TRACE, name = "db_highlight")]
    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        if self.0 == SearchMode::Prefix {
            return vec![];
        } else if self.0 == SearchMode::FullText {
            return get_highlight_indices_fulltext(command, search_input);
        }
        let mut fzf = FzfV2::new();
        let mut parser = FzfParser::new();
        let query = parser.parse(search_input);
        let mut ranges: Vec<Range<usize>> = Vec::new();
        let _ = fzf.distance_and_ranges(query, command, &mut ranges);

        // convert ranges to all indices
        ranges.into_iter().flatten().collect()
    }
}

#[instrument(skip_all, level = Level::TRACE, name = "db_highlight_fulltext")]
pub fn get_highlight_indices_fulltext(command: &str, search_input: &str) -> Vec<usize> {
    let mut ranges = vec![];
    let lower_command = command.to_ascii_lowercase();

    for token in QueryTokenizer::new(search_input) {
        let matchee = if token.has_uppercase() {
            command
        } else {
            &lower_command
        };

        if token.is_inverse() {
            continue;
        }

        match token {
            QueryToken::Or => {}
            QueryToken::Regex(r) => {
                if let Ok(re) = regex::Regex::new(r) {
                    for m in re.find_iter(command) {
                        ranges.push(m.range());
                    }
                }
            }
            QueryToken::MatchStart(term, _) => {
                if matchee.starts_with(term) {
                    ranges.push(0..term.len());
                }
            }
            QueryToken::MatchEnd(term, _) => {
                if matchee.ends_with(term) {
                    let l = matchee.len();
                    ranges.push((l - term.len())..l);
                }
            }
            QueryToken::Match(term, _) | QueryToken::MatchFull(term, _) => {
                for (idx, m) in matchee.match_indices(term) {
                    ranges.push(idx..(idx + m.len()));
                }
            }
        }
    }

    let mut ret: Vec<_> = ranges.into_iter().flatten().collect();
    ret.sort_unstable();
    ret.dedup();
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::client::search::cursor::Cursor;
    use atuin_client::{
        database::{Context, Database, Sqlite},
        history::{AUTHOR_FILTER_ALL_USER, History},
        settings::FilterMode,
    };
    use time::macros::datetime;

    fn context() -> Context {
        Context {
            session: uuid::Uuid::now_v7().as_simple().to_string(),
            cwd: "/tmp".to_string(),
            hostname: "host:user".to_string(),
            host_id: String::new(),
            git_root: None,
        }
    }

    #[tokio::test]
    async fn empty_query_uses_author_filters() {
        let mut db = Sqlite::new(":memory:", 0.1).await.unwrap();

        let user_history: History = History::import()
            .timestamp(datetime!(2024-01-01 10:00 UTC))
            .command("git status")
            .cwd("/tmp")
            .author("ellie")
            .build()
            .into();
        let agent_history: History = History::import()
            .timestamp(datetime!(2024-01-01 11:00 UTC))
            .command("git diff")
            .cwd("/tmp")
            .author("codex")
            .build()
            .into();

        db.save_bulk(&[user_history.clone(), agent_history])
            .await
            .unwrap();

        let mut engine = Search(SearchMode::Fuzzy);
        let state = SearchState {
            input: Cursor::from(String::new()),
            filter_mode: FilterMode::Global,
            context: context(),
            custom_context: None,
            authors: vec![AUTHOR_FILTER_ALL_USER.to_string()],
        };

        let results = engine.query(&state, &mut db).await.unwrap();

        assert_eq!(results, vec![user_history]);
    }
}
