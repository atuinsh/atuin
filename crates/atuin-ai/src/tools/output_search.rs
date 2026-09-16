//! `atuin_output_search`: full-text search over captured command output.

use std::collections::BTreeSet;
use std::ops::Range;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::string::highlighted::Piece;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::client::SearchClient;
use eyre::Result;
use futures::TryStreamExt;
use serde::{Deserialize, Deserializer};

use super::{NO_OUTPUT_ADVICE, ToolOutcome};
use crate::history_format::format_history_search_result;

/// Page-size bounds for `atuin_output_search`; mirrored in the MCP schema.
pub const DEFAULT_OUTPUT_SEARCH_RESULTS: u32 = 5;
pub const MAX_OUTPUT_SEARCH_RESULTS: u32 = 20;

/// Output lines shown on each side of a matching line.
const CONTEXT_LINES: usize = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct AtuinOutputSearchToolCall {
    pub query: String,
    #[serde(default = "default_limit", deserialize_with = "deserialize_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    DEFAULT_OUTPUT_SEARCH_RESULTS
}

/// Models often send `null` for optional params, so it counts as omitted; anything else is
/// clamped into the schema's bounds rather than rejected.
fn deserialize_limit<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let limit = Option::<u32>::deserialize(deserializer)?;
    Ok(limit.map_or(DEFAULT_OUTPUT_SEARCH_RESULTS, |l| l.clamp(1, MAX_OUTPUT_SEARCH_RESULTS)))
}

impl TryFrom<&serde_json::Value> for AtuinOutputSearchToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let mut call = Self::deserialize(value)?;
        call.query = call.query.trim().to_string();
        if call.query.is_empty() {
            eyre::bail!("query must not be blank");
        }
        Ok(call)
    }
}

impl AtuinOutputSearchToolCall {
    pub(crate) async fn execute(&self, db: &Sqlite) -> ToolOutcome {
        let settings = match Settings::new() {
            Ok(settings) => settings,
            Err(e) => return ToolOutcome::Error(format!("Failed to load Atuin settings: {e}")),
        };
        if settings.output.limits().is_none() {
            return ToolOutcome::Error(
                "Output search is unavailable: output capture is disabled in the Atuin config \
                 (the [output] section), so no command output has been recorded. History search \
                 still works."
                    .to_string(),
            );
        }

        let mut client = match SearchClient::from_settings(&settings).await {
            Ok(client) => client,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "Output search is unavailable: could not connect to the Atuin daemon ({e}). \
                     History search still works. {NO_OUTPUT_ADVICE}"
                ));
            }
        };

        // 0 = unbounded: the daemon streams by relevance and we stop once `limit` hits have been
        // rendered, so hits without a local history entry never starve the page.
        let matches = match client.search_command_output(self.query.clone(), 0).await {
            Ok(matches) => matches,
            Err(e) => return ToolOutcome::Error(format!("Output search failed: {e}")),
        };
        let mut matches = std::pin::pin!(matches);

        let local_offset = time::UtcOffset::local_or_utc();
        let mut formatted = Vec::new();
        while formatted.len() < self.limit as usize {
            let m = match matches.try_next().await {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => return ToolOutcome::Error(format!("Output search failed: {e}")),
            };
            let history = match db.load(m.history_id).await {
                Ok(Some(history)) => history,
                Ok(None) => continue,
                Err(e) => return ToolOutcome::Error(format!("Failed to load history: {e}")),
            };

            let mut plain = String::new();
            let mut ranges = Vec::new();
            for piece in m.output.pieces() {
                match piece {
                    Piece::Text(text) => plain.push_str(text),
                    Piece::Match(text) => {
                        let start = plain.len();
                        plain.push_str(text);
                        ranges.push(start..plain.len());
                    }
                }
            }

            formatted.push(format!(
                "{}Matching output lines:\n{}\n",
                format_history_search_result(formatted.len() + 1, &history, local_offset),
                matching_lines(&plain, &ranges, CONTEXT_LINES),
            ));
        }

        if formatted.is_empty() {
            return ToolOutcome::Success(format!(
                "No captured output matched query {:?}. Only commands run in an Atuin-enabled \
                 terminal while the daemon was running are searchable, and older output may have \
                 been dropped. Terms are AND-ed and matched as whole words, so try fewer or \
                 different terms.",
                self.query
            ));
        }
        ToolOutcome::Success(formatted.join("\n"))
    }
}

/// Render only the lines of `plain` that overlap a match range, plus `context` lines on either
/// side, numbered 1-based (so they line up with `atuin_output` ranges) and with `[...]` where
/// lines were skipped. `ranges` are byte ranges into `plain`, ascending.
fn matching_lines(plain: &str, ranges: &[Range<usize>], context: usize) -> String {
    let lines: Vec<&str> = plain.split_inclusive('\n').collect();

    let mut selected = BTreeSet::new();
    let mut ranges = ranges.iter().filter(|r| !r.is_empty()).peekable();
    let mut line_start = 0;
    for (idx, line) in lines.iter().enumerate() {
        let line_end = line_start + line.len();
        while ranges.peek().is_some_and(|r| r.end <= line_start) {
            ranges.next();
        }
        if ranges.peek().is_some_and(|r| r.start < line_end) {
            selected.extend(idx.saturating_sub(context)..=(idx + context).min(lines.len() - 1));
        }
        line_start = line_end;
    }

    let width = selected.last().map_or(0, |last| (last + 1).to_string().len());
    let mut out = Vec::new();
    let mut previous = None;
    for idx in selected {
        if previous.is_some_and(|p| idx > p + 1) {
            out.push("[...]".to_string());
        }
        let content = lines[idx].strip_suffix('\n').unwrap_or(lines[idx]);
        out.push(format!("{:>width$}\t{content}", idx + 1));
        previous = Some(idx);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case::default(json!({"query": "disk full"}), 5)]
    #[case::clamped_high(json!({"query": "disk", "limit": 100}), 20)]
    #[case::clamped_low(json!({"query": "disk", "limit": 0}), 1)]
    #[case::null_limit(json!({"query": "disk", "limit": null}), 5)]
    fn parses_query_and_clamps_limit(#[case] input: serde_json::Value, #[case] limit: u32) {
        let call = AtuinOutputSearchToolCall::try_from(&input).unwrap();
        assert_eq!(call.query, input["query"].as_str().unwrap());
        assert_eq!(call.limit, limit);
    }

    #[rstest]
    #[case::missing(json!({}))]
    #[case::empty(json!({"query": ""}))]
    #[case::blank(json!({"query": "   "}))]
    #[case::not_a_string(json!({"query": 3}))]
    fn rejects_a_blank_query(#[case] input: serde_json::Value) {
        assert!(AtuinOutputSearchToolCall::try_from(&input).is_err());
    }

    fn ranges(plain: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
        plain.match_indices(needle).map(|(i, m)| i..i + m.len()).collect()
    }

    #[rstest]
    #[case::context_around_a_middle_line("a\nb\nerror\nc\nd", "error", "2\tb\n3\terror\n4\tc")]
    #[case::no_context_before_the_first_line("error\nb\nc", "error", "1\terror\n2\tb")]
    #[case::adjacent_windows_merge(
        "error\nb\nerror\nd\ne",
        "error",
        "1\terror\n2\tb\n3\terror\n4\td"
    )]
    #[case::distant_windows_are_separated(
        "error\nb\nc\nd\nerror\nf",
        "error",
        "1\terror\n2\tb\n[...]\n4\td\n5\terror\n6\tf"
    )]
    #[case::two_hits_on_one_line_show_it_once(
        "x\nerror error\ny",
        "error",
        "1\tx\n2\terror error\n3\ty"
    )]
    #[case::line_numbers_align("a\nb\nc\nd\ne\nf\ng\nh\ni\nerror", "error", " 9\ti\n10\terror")]
    fn matching_lines_show_hits_with_context(
        #[case] plain: &str,
        #[case] needle: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(matching_lines(plain, &ranges(plain, needle), 1), expected);
    }
}
