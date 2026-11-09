use std::ops::Range;

use atuin_client::database::{DbSearchMode, OptFilters, QueryToken, QueryTokenizer, Sqlite};
use atuin_client::history::{History, all_user_author_filter};
use atuin_client::settings::CaseMatching;
use eyre::Result;
use norm::fzf::{FzfParser, FzfV2};
use norm::{CaseSensitivity, Metric};
use tracing::{Level, instrument};

use super::{SearchEngine, SearchState};

pub struct Search(pub DbSearchMode, pub CaseMatching);

impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "db_search", fields(mode = ?self.0, query = %state.input.as_str()))]
    async fn full_query(&mut self, state: &SearchState, db: &mut Sqlite) -> Result<Vec<History>> {
        let shells = state.shells.to_filter();
        let results = db
            .search(self.0, state.filter_mode, &state.context, state.input.as_str(), OptFilters {
                limit: Some(200),
                authors: all_user_author_filter(),
                shells: shells.as_filter(),
                case_matching: self.1,
                ..Default::default()
            })
            .await
            // ignore errors as it may be caused by incomplete regex
            .map_or(Vec::new(), |r| r.into_iter().collect());
        Ok(results)
    }

    #[instrument(skip_all, level = Level::TRACE, name = "db_highlight")]
    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        if self.0 == DbSearchMode::Prefix {
            return vec![];
        } else if self.0 == DbSearchMode::FullText {
            return get_highlight_indices_fulltext(command, search_input, self.1);
        }
        let mut fzf = FzfV2::new();
        fzf.set_case_sensitivity(match self.1 {
            CaseMatching::Smart => CaseSensitivity::Smart,
            CaseMatching::Ignore => CaseSensitivity::Insensitive,
            CaseMatching::Respect => CaseSensitivity::Sensitive,
        });
        let mut parser = FzfParser::new();
        let query = parser.parse(search_input);
        let mut ranges: Vec<Range<usize>> = Vec::new();
        let _ = fzf.distance_and_ranges(query, command, &mut ranges);

        // convert ranges to all indices
        ranges.into_iter().flatten().collect()
    }
}

#[instrument(skip_all, level = Level::TRACE, name = "db_highlight_fulltext")]
pub fn get_highlight_indices_fulltext(
    command: &str,
    search_input: &str,
    case_matching: CaseMatching,
) -> Vec<usize> {
    let mut ranges = vec![];
    let lower_command = command.to_ascii_lowercase();

    for token in QueryTokenizer::new(search_input) {
        if token.is_inverse() {
            continue;
        }

        // ASCII-only lowercasing keeps byte offsets lined up with `command`.
        let sensitive = token.is_case_sensitive(case_matching);
        let matchee = if sensitive {
            command
        } else {
            &lower_command
        };
        let fold = |term: &str| {
            if sensitive {
                term.to_owned()
            } else {
                term.to_ascii_lowercase()
            }
        };

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
                let term = fold(term);
                if matchee.starts_with(&term) {
                    ranges.push(0..term.len());
                }
            }
            QueryToken::MatchEnd(term, _) => {
                let term = fold(term);
                if matchee.ends_with(&term) {
                    let l = matchee.len();
                    ranges.push((l - term.len())..l);
                }
            }
            QueryToken::Match(term, _) | QueryToken::MatchFull(term, _) => {
                for (idx, m) in matchee.match_indices(&fold(term)) {
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
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::smart_lowercase(CaseMatching::Smart, "ellie", vec![9, 10, 11, 12, 13])]
    #[case::smart_uppercase(CaseMatching::Smart, "Ellie", vec![9, 10, 11, 12, 13])]
    #[case::smart_wrong_case(CaseMatching::Smart, "ELLIE", vec![])]
    #[case::ignore_wrong_case(CaseMatching::Ignore, "ELLIE", vec![9, 10, 11, 12, 13])]
    #[case::ignore_start(CaseMatching::Ignore, "^CD", vec![0, 1])]
    #[case::respect_wrong_case(CaseMatching::Respect, "ellie", vec![])]
    #[case::respect_right_case(CaseMatching::Respect, "Ellie", vec![9, 10, 11, 12, 13])]
    fn fulltext_highlight_follows_case_matching(
        #[case] case_matching: CaseMatching,
        #[case] query: &str,
        #[case] expected: Vec<usize>,
    ) {
        let search = Search(DbSearchMode::FullText, case_matching);
        assert_eq!(search.get_highlight_indices("cd /home/Ellie", query), expected);
    }

    #[rstest]
    #[case::smart(CaseMatching::Smart, true)]
    #[case::ignore(CaseMatching::Ignore, true)]
    #[case::respect(CaseMatching::Respect, false)]
    fn fuzzy_highlight_follows_case_matching(
        #[case] case_matching: CaseMatching,
        #[case] highlighted: bool,
    ) {
        let search = Search(DbSearchMode::Fuzzy, case_matching);
        assert_eq!(!search.get_highlight_indices("GIT STATUS", "git").is_empty(), highlighted);
    }
}
