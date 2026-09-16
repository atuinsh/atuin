use std::ops::Range;

use atuin_client::database::{DbSearchMode, OptFilters, QueryToken, QueryTokenizer, Sqlite};
use atuin_client::history::{History, all_user_author_filter};
use eyre::Result;
use norm::Metric;
use norm::fzf::{FzfParser, FzfV2};
use tracing::{Level, instrument};

use super::{SearchEngine, SearchState};

pub struct Search(pub DbSearchMode);

impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "db_search", fields(mode = ?self.0, query = %state.input.as_str()))]
    async fn full_query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>> {
        let shells = state.shells.to_filter();
        let results = db
            .search(self.0, state.filter_mode, &state.context, state.input.as_str(), OptFilters {
                limit: Some(200),
                authors: all_user_author_filter(),
                shells: shells.as_filter(),
                ..Default::default()
            })
            .await
            // ignore errors as it may be caused by incomplete regex
            .map_or(Vec::new(), |r| r.into_iter().collect());
        Ok(results)
    }
}

impl Search {
    /// Build the query-invariant highlighter state once, so the per-row work
    /// (`highlight_indices`) reuses the fzf scorer + parser instead of
    /// rebuilding them for every visible row on every frame.
    pub fn prepare_highlighter(&self, search_input: &str) -> DbHighlighter {
        // Empty input can never highlight anything: skip building any matcher.
        if search_input.is_empty() || self.0 == DbSearchMode::Prefix {
            return DbHighlighter::Empty;
        }
        if self.0 == DbSearchMode::FullText {
            return DbHighlighter::FullText(search_input.to_owned());
        }
        // `FzfV2` heap-allocates ~5kb of scoring scratch; hoisting it out of
        // the per-row loop is the whole point of this type. Boxed because it
        // dwarfs the other variants.
        DbHighlighter::Fuzzy(Box::new(FuzzyHighlighter {
            fzf: FzfV2::new(),
            parser: FzfParser::new(),
            query: search_input.to_owned(),
        }))
    }
}

/// Query-invariant highlighter state, built once per render frame.
pub enum DbHighlighter {
    /// Prefix mode or empty input — nothing is ever highlighted.
    Empty,
    FullText(String),
    Fuzzy(Box<FuzzyHighlighter>),
}

pub struct FuzzyHighlighter {
    fzf: FzfV2,
    parser: FzfParser,
    query: String,
}

impl DbHighlighter {
    #[instrument(skip_all, level = Level::TRACE, name = "db_highlight")]
    pub fn highlight_indices(&mut self, command: &str) -> Vec<usize> {
        match self {
            Self::Empty => Vec::new(),
            Self::FullText(query) => get_highlight_indices_fulltext(command, query),
            Self::Fuzzy(state) => {
                let state = &mut **state;
                // `FzfQuery` borrows `parser`, so it is re-parsed per row; the
                // parse reuses the parser's buffers and is cheap next to the
                // scorer allocation this type hoists out.
                let parsed = state.parser.parse(&state.query);
                let mut ranges: Vec<Range<usize>> = Vec::new();
                let _ = state.fzf.distance_and_ranges(parsed, command, &mut ranges);
                ranges.into_iter().flatten().collect()
            }
        }
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
