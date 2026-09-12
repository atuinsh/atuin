use std::ops::Range;

use atuin_client::database::{DbSearchMode, OptFilters, QueryToken, QueryTokenizer, Sqlite};
use atuin_client::history::History;
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
                authors: state.authors.as_slice_filter(),
                shells: shells.as_filter(),
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
    use std::time::Duration;

    use atuin_client::database::{Context, DbSearchMode, Sqlite};
    use atuin_client::history::{AuthorPattern, History};
    use atuin_client::settings::{FilterMode, Shells};
    use atuin_common::filter::OrFilter;
    use atuin_domain::record::CmdOrigin;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::{Search, SearchEngine, SearchState};

    #[rstest]
    #[tokio::test]
    async fn interactive_search_applies_author_and_shell_filters(
        #[values("", "echo")] input: &str,
    ) {
        let mut db = Sqlite::in_memory(Duration::from_secs(2)).await.unwrap();
        for (command, author, shell) in [
            ("echo user zsh", "alice", "zsh"),
            ("echo agent bash", "codex", "bash"),
            ("echo agent zsh", "codex", "zsh"),
        ] {
            let history: History = History::capture()
                .timestamp(OffsetDateTime::now_utc())
                .command(command)
                .cwd("/tmp")
                .author(author)
                .shell(shell)
                .build()
                .into();
            db.save(&history).await.unwrap();
        }

        let mut engine = Search(DbSearchMode::FullText);
        let state = SearchState {
            input: input.to_owned().into(),
            filter_mode: FilterMode::Global,
            context: Context {
                session: "session".into(),
                cwd: "/tmp".into(),
                cmd_origin: CmdOrigin::default(),
                host_id: "host".into(),
                git_root: None,
            },
            custom_context: None,
            authors: OrFilter::from_list(vec![AuthorPattern::AllAgent]).unwrap(),
            shells: Shells::Fixed(OrFilter::from_list(vec!["zsh".to_owned()]).unwrap()),
        };

        let results = engine.full_query(&state, &mut db).await.unwrap();

        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0].command, "echo agent zsh");
    }
}
