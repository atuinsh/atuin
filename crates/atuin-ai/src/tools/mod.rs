use atuin_client::history::{AuthorPattern, History};
use atuin_client::settings::FilterMode;
use atuin_common::filter::OrFilter;
use eyre::Result;
use serde::Deserialize;
use strum_macros::{EnumIter, EnumString, IntoStaticStr};

/// Page-size bounds for `atuin_history`; mirrored in the MCP schema.
pub const DEFAULT_HISTORY_RESULTS: i64 = 10;
pub const MAX_HISTORY_RESULTS: i64 = 50;
pub mod output;
pub mod search_terms;
pub mod session;

pub use output::get::AtuinOutputToolCall;

/// What a tool says back: its text, or an error to show the model.
pub type ToolOutcome = Result<String, String>;

#[derive(Debug, Clone)]
pub struct AtuinHistoryToolCall {
    pub filter_modes: Vec<HistorySearchFilterMode>,
    pub query: String,
    pub limit: i64,
    pub only_failed: bool,
    pub authors: OrFilter<Vec<AuthorPattern>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum HistorySearchFilterMode {
    Global,
    Host,
    Session,
    Directory,
    Workspace,
}

impl HistorySearchFilterMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

impl From<HistorySearchFilterMode> for FilterMode {
    fn from(mode: HistorySearchFilterMode) -> Self {
        match mode {
            HistorySearchFilterMode::Global => Self::Global,
            HistorySearchFilterMode::Host => Self::Host,
            HistorySearchFilterMode::Session => Self::Session,
            HistorySearchFilterMode::Directory => Self::Directory,
            HistorySearchFilterMode::Workspace => Self::Workspace,
        }
    }
}

/// `atuin_history`'s arguments as sent. An argument the schema doesn't list is an error rather
/// than ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryArgs {
    query: String,
    filter_modes: Option<Vec<String>>,
    limit: Option<i64>,
    only_failed: Option<bool>,
    authors: Option<Vec<String>>,
}

impl TryFrom<&serde_json::Value> for AtuinHistoryToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        // Every field but `query` is optional, and JSON null counts as omitted because models
        // often send null for optional params. A missing scope means a global search — evals
        // showed that forcing the model to pick a scope on every call made it stop calling the
        // tool at all.
        let args = HistoryArgs::deserialize(value)?;

        let filter_modes = args
            .filter_modes
            .unwrap_or_default()
            .iter()
            .map(|mode| {
                mode.parse::<HistorySearchFilterMode>()
                    .map_err(|_| eyre::eyre!("Invalid filter mode: {mode}"))
            })
            .collect::<Result<Vec<HistorySearchFilterMode>>>()?;

        // An omitted or empty `authors` array means no author filtering.
        let authors = args.authors.unwrap_or_default().into_iter().map(AuthorPattern::from);
        let authors = OrFilter::from_list(authors.collect::<Vec<_>>()).unwrap_or_default();

        Ok(Self {
            filter_modes,
            query: args.query,
            limit: args.limit.unwrap_or(DEFAULT_HISTORY_RESULTS).clamp(1, MAX_HISTORY_RESULTS),
            only_failed: args.only_failed.unwrap_or(false),
            authors,
        })
    }
}

/// Top `results` up to `size` with `loose` matches for commands not already
/// in it, keeping their order.
fn fill_with_loose(results: &mut Vec<History>, loose: Vec<History>, size: usize) {
    for history in loose {
        if results.len() >= size {
            break;
        }
        if !results.iter().any(|r| r.command == history.command) {
            results.push(history);
        }
    }
}

impl AtuinHistoryToolCall {
    pub(crate) async fn execute(
        &self,
        db: &atuin_client::database::Sqlite,
        settings: &atuin_client::settings::Settings,
    ) -> ToolOutcome {
        use atuin_client::database::{self, DbSearchMode, OptFilters};

        // query_context rather than current_context: when running outside an
        // atuin-hooked shell (e.g. as an MCP server) there is no ATUIN_SESSION.
        let context = match database::query_context(settings).await {
            Ok(ctx) => ctx,
            Err(e) => return Err(format!("Failed to get history context: {e}")),
        };

        let search_mode =
            self.filter_modes.first().copied().unwrap_or(HistorySearchFilterMode::Global);

        // An empty session would silently match nothing; error instead so a
        // missing $ATUIN_SESSION (e.g. MCP server launched outside a hooked
        // shell) isn't mistaken for empty history.
        if matches!(search_mode, HistorySearchFilterMode::Session) && context.session.is_empty() {
            return Err("Session-scoped search is unavailable: $ATUIN_SESSION is not set, so \
                        there is no shell session to scope to. Use another filter mode."
                .to_string());
        }

        let filter_options = OptFilters {
            // Fetch one row beyond the requested limit so the truncation
            // notice appended to the results below can say "more exist" as a
            // fact rather than a guess.
            limit: Some(self.limit + 1),
            only_failed: self.only_failed,
            authors: self.authors.as_slice_filter(),
            ..Default::default()
        };

        // Each term as a substring first: fuzzy matching alone takes the most
        // recent commands with the letters anywhere in order, so a short query
        // like `pscale` is buried under long heredocs that happen to spell it
        // out. Fuzzy matches only fill what is left of the page.
        let search = |mode| {
            db.search(mode, search_mode.into(), &context, &self.query, filter_options.clone())
        };
        // Full-text results come back newest first; rank them as fuzzy ones
        // are, tightest match first.
        let mut results = match search(DbSearchMode::FullText).await {
            Ok(results) => atuin_client::ordering::reorder_fuzzy(
                DbSearchMode::Fuzzy,
                &database::ranking_query(&self.query),
                results,
            ),
            Err(e) => return Err(format!("History search failed: {e}")),
        };
        // The clamp keeps this in 1..=MAX_HISTORY_RESULTS, so the conversion
        // never fails; the fallback only exists to keep it infallible.
        let page_size = usize::try_from(self.limit.clamp(1, MAX_HISTORY_RESULTS)).unwrap_or(1);
        let exact = results.len();
        // A page with room, or exactly full (to know whether more exist), is
        // topped up; a query of operators alone (`^prefix`, `'exact`, regexes)
        // matches the same either way.
        if exact <= page_size && database::has_fuzzy_terms(&self.query) {
            let loose = match search(DbSearchMode::Fuzzy).await {
                Ok(loose) => loose,
                Err(e) => return Err(format!("History search failed: {e}")),
            };
            fill_with_loose(&mut results, loose, page_size + 1);
        }
        let truncated = results.len() > page_size;
        results.truncate(page_size);

        if results.is_empty() {
            // An unadorned "no results" reads to the model as "this tool is
            // useless". List which search parameters are worth loosening so
            // its retry has somewhere to go.
            let mut hints = Vec::new();
            if !self.query.is_empty() {
                hints.push("query terms are AND-ed, so try fewer or shorter terms");
            }
            if !matches!(search_mode, HistorySearchFilterMode::Global) {
                hints.push("widen the scope to 'global'");
            }
            if self.only_failed {
                hints.push("drop only_failed (the command may have succeeded)");
            }
            if !self.authors.is_all() {
                hints.push("drop the authors filter");
            }
            let mut msg = format!(
                "No history entries matched query {query:?} (scope: {scope}).",
                query = self.query,
                scope = search_mode.as_str(),
            );
            if !hints.is_empty() {
                msg.push_str(&format!(" To find more: {}.", hints.join("; ")));
            }
            return Ok(msg);
        }

        let local_tz = jiff::tz::TimeZone::system();

        let mut formatted: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, history)| {
                crate::history_format::format_history_search_result(i + 1, history, &local_tz)
            })
            .collect();
        if exact < results.len() {
            let which = if exact == 0 {
                "No command contains the query as written; these"
            } else {
                "The rest"
            };
            formatted.insert(
                exact,
                format!(
                    "[{which} only match fuzzily: the query's letters appear in order, not \
                     necessarily together.]"
                ),
            );
        }

        if truncated {
            // The parser clamps `limit` to MAX_HISTORY_RESULTS, so a model
            // that retries with a larger limit at the cap would just get the
            // identical page back; only suggest raising it below the cap.
            let advice = if self.limit < MAX_HISTORY_RESULTS {
                "Refine the query or raise `limit` to see others."
            } else {
                "That is the maximum page size; refine the query to narrow them."
            };
            formatted
                .push(format!("[Showing the first {page_size} matches; more exist. {advice}]"));
        }

        Ok(formatted.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter;
    use rstest::*;

    use super::*;

    fn history(command: &str) -> History {
        History::capture()
            .timestamp(jiff::Timestamp::UNIX_EPOCH)
            .command(command)
            .cwd("/")
            .build()
            .into()
    }

    fn commands(results: &[History]) -> Vec<&str> {
        results.iter().map(|h| h.command.as_str()).collect()
    }

    /// Substring matches lead; fuzzy ones fill the rest of the page without
    /// repeating a command.
    #[rstest]
    #[case::fills(3, vec!["pscale connect", "cat <<EOF p s c a l e", "pscale shell"])]
    #[case::stops_at_size(2, vec!["pscale connect", "cat <<EOF p s c a l e"])]
    #[case::exact_fills_the_page(1, vec!["pscale connect"])]
    fn fuzzy_matches_fill_after_substring_ones(#[case] size: usize, #[case] want: Vec<&str>) {
        let mut results = vec![history("pscale connect")];
        let loose = ["pscale connect", "cat <<EOF p s c a l e", "pscale shell", "pip scale"];
        fill_with_loose(&mut results, loose.into_iter().map(history).collect(), size);
        assert_eq!(commands(&results), want);
    }

    /// Substring matches rank tightest first, as fuzzy ones did, not newest
    /// first.
    #[rstest]
    fn substring_matches_rank_by_tightness() {
        use atuin_client::database::{DbSearchMode, ranking_query};
        let newest_first = ["ffmpeg -i in.mkv out.mkv # convert later to av1", "ffmpeg -c:v av1 x"];
        let ranked = atuin_client::ordering::reorder_fuzzy(
            DbSearchMode::Fuzzy,
            &ranking_query("ffmpeg av1"),
            newest_first.into_iter().map(history).collect(),
        );
        assert_eq!(commands(&ranked), ["ffmpeg -c:v av1 x", newest_first[0]]);
    }

    #[rstest]
    #[case::omitted(serde_json::json!({ "query": "cargo" }))]
    #[case::null(serde_json::json!({ "query": "cargo", "filter_modes": null }))]
    fn atuin_history_filter_modes_are_optional(#[case] input: serde_json::Value) {
        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(call.filter_modes.is_empty());
    }

    #[rstest]
    fn filter_mode_names_round_trip() {
        use strum::IntoEnumIterator;
        for mode in HistorySearchFilterMode::iter() {
            assert_eq!(mode.as_str().parse::<HistorySearchFilterMode>(), Ok(mode));
        }
    }

    #[rstest]
    fn atuin_history_filter_modes_reject_non_arrays() {
        let input = serde_json::json!({ "query": "cargo", "filter_modes": "global" });
        assert!(AtuinHistoryToolCall::try_from(&input).is_err());
    }

    #[rstest]
    fn atuin_history_rejects_unknown_arguments() {
        let input = serde_json::json!({ "query": "cargo", "since": "2h" });
        let err = AtuinHistoryToolCall::try_from(&input).unwrap_err().to_string();
        assert!(err.contains("`since`") && err.contains("`only_failed`"), "{err}");
    }

    #[rstest]
    fn atuin_history_author_and_failure_filters_parse() {
        let input = serde_json::json!({ "query": "cargo" });

        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(!call.only_failed);
        assert!(call.authors.is_all());

        let input = serde_json::json!({
            "query": "cargo",
            "only_failed": true,
            "authors": ["$all-agent"],
        });

        let call = AtuinHistoryToolCall::try_from(&input).unwrap();
        assert!(call.only_failed);
        assert_eq!(call.authors.items(), filter::Items::Some([AuthorPattern::AllAgent].as_slice()));
    }
}
