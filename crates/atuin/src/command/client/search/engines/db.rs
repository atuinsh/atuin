use super::{SearchEngine, SearchState};
use async_trait::async_trait;
use atuin_client::{
    database::Database,
    database::OptFilters,
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
        let results = db
            .search(
                self.0,
                state.filter_mode,
                &state.context,
                state.input.as_str(),
                OptFilters {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .await
            // ignore errors as it may be caused by incomplete regex
            .map_or(Vec::new(), |r| r.into_iter().collect());
        Ok(results)
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
