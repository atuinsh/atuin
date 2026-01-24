use super::{SearchEngine, SearchState};
use async_trait::async_trait;
use atuin_client::{
    database::Database, database::OptFilters, history::History, settings::SearchMode,
};
use eyre::Result;
use norm::Metric;
use norm::fzf::{FzfParser, FzfV2};
use std::ops::Range;

pub struct Search(pub SearchMode);

#[async_trait]
impl SearchEngine for Search {
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        Ok(db
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
            .map_or(Vec::new(), |r| r.into_iter().collect()))
    }

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

fn get_highlight_indices_fulltext(command: &str, search_input: &str) -> Vec<usize> {
    let mut ranges = vec![];
    let mut is_regex = false;
    let uncased_command = command.to_ascii_lowercase();
    for part in search_input.split_inclusive(' ') {
        let match_part = match (is_regex, part.starts_with("r/")) {
            (false, false) => {
                if part.trim_end().is_empty() {
                    continue;
                }
                part.trim_end()
            }
            (false, true) => {
                is_regex = true;
                continue;
            }
            (true, _) => {
                if part.trim_end().ends_with('/') {
                    is_regex = false;
                }
                continue;
            }
        };

        if match_part.starts_with('!') {
            continue;
        };

        let case_sensitive = match_part.contains(char::is_uppercase);
        let mut at_start = false;
        let mut at_end = false;

        let match_part = if let Some(term) = match_part.strip_prefix('^') {
            at_start = true;
            term
        } else if let Some(term) = match_part.strip_suffix('$') {
            at_end = true;
            term
        } else {
            match_part
        };

        let matchee = if case_sensitive {
            command
        } else {
            &uncased_command
        };
        if at_start && matchee.starts_with(match_part) {
            ranges.push(0..match_part.len());
        } else if at_end && matchee.ends_with(match_part) {
            let l = matchee.len();
            ranges.push((l - match_part.len())..l);
        } else {
            for (idx, m) in matchee.match_indices(match_part) {
                ranges.push(idx..(idx + m.len()));
            }
        }
    }

    let mut ret: Vec<_> = ranges.into_iter().flatten().collect();
    ret.sort_unstable();
    ret.dedup();
    ret
}
