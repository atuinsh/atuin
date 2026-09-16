//! `atuin_output_search`: full-text search over captured command output.

use std::ops::Range;

use atuin_client::database::Sqlite;
use atuin_client::settings::{OutputCapture, Settings};
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::string::highlighted::Plain;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::client::SearchClient;
use atuin_daemon::grpc::history::pb::ChunkedOutputLineView;
use easy_cast::Conv;
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{NO_OUTPUT_ADVICE, format_chunked_output_line_views_for_llm};
use crate::history_format::format_history_search_result;
use crate::tools::ToolOutcome;

/// Output lines shown on each side of a matching line.
const CONTEXT_LINES: usize = 1;

// Doc comments on the fields are the descriptions the model reads in the tool schema; the
// struct deliberately has none, as it would become the schema's top-level description.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinOutputSearchToolCall {
    /// Words to look for in captured command output. Terms are AND-ed and matched as whole
    /// words (case-insensitive; no regex, no prefix matching), so use a few distinctive words
    /// from the text you remember, e.g. 'connection refused' or 'ENOSPC', not a sentence.
    pub query: NonBlankString,
    /// Maximum number of commands to return, most relevant first.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 20, 5>,
}

impl AtuinOutputSearchToolCall {
    pub(crate) async fn execute(&self, db: &Sqlite, settings: &Settings) -> ToolOutcome {
        if matches!(settings.output, OutputCapture::Disabled) {
            return ToolOutcome::Error(
                "Output search is unavailable: output capture is disabled in the Atuin config \
                 (the [output] section), so no command output has been recorded. History search \
                 still works."
                    .to_string(),
            );
        }

        // TODO(markovejnovic): It would be good if this was injected into the tool rather than
        //                      built ad-hoc. However, the anti-pattern already exists, and I'd like
        //                      to keep the ball rolling.
        let mut client = match SearchClient::from_settings(settings).await {
            Ok(client) => client,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "Output search is unavailable: could not connect to the Atuin daemon ({e}). \
                     History search still works. {NO_OUTPUT_ADVICE}"
                ));
            }
        };

        let hits = async {
            client
                .search_command_output(self.query.to_string(), None)
                .await
                .map_err(|e| format!("Output search failed: {e}"))?
                .map_err(|e| format!("Output search failed: {e}"))
                .try_filter_map(|m| async move {
                    db.load(m.history_id)
                        .await
                        .map(|history| history.map(|history| (history, m)))
                        .map_err(|e| format!("Failed to load history: {e}"))
                })
                .take(self.limit.get() as usize)
                .try_collect::<Vec<_>>()
                .await
        }
        .await;
        let hits = match hits {
            Ok(hits) => hits,
            Err(e) => return ToolOutcome::Error(e),
        };

        let local_offset = time::UtcOffset::local_or_utc();
        let formatted: Vec<String> = hits
            .iter()
            .enumerate()
            .map(|(i, (history, m))| {
                let plain = m.output.to_plain();
                format!(
                    "{}Matching output lines:\n{}\n",
                    format_history_search_result(i + 1, history, local_offset),
                    format_chunked_output_line_views_for_llm(
                        matching_lines(&plain, CONTEXT_LINES).into_iter()
                    ),
                )
            })
            .collect();

        if formatted.is_empty() {
            return ToolOutcome::Success(format!(
                "No captured output matched query {:?}. Only commands run in an Atuin-enabled \
                 terminal while the daemon was running are searchable, and older output may have \
                 been dropped. Terms are AND-ed and matched as whole words, so try fewer or \
                 different terms.",
                self.query.trim()
            ));
        }
        ToolOutcome::Success(formatted.join("\n"))
    }
}

/// `grep -C context` over `plain`: the lines overlapping a match, with context.
fn matching_lines<'p>(plain: &'p Plain<'_>, context: usize) -> Vec<ChunkedOutputLineView<'p>> {
    /// Whether `span` intersects any of `ranges` (non-empty, ascending).
    fn overlaps(ranges: &[Range<usize>], span: &Range<usize>) -> bool {
        let next = ranges.partition_point(|r| r.end <= span.start);
        ranges.get(next).is_some_and(|r| r.start < span.end)
    }

    /// Coalesce ascending ranges that overlap or touch.
    fn merge(ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
        ranges.fold(Vec::new(), |mut merged, r| {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
            merged
        })
    }

    let lines: Vec<&str> = plain.text.split_inclusive('\n').collect();
    let windows = merge(
        lines
            .iter()
            .scan(0, |start, line| {
                let span = *start..*start + line.len();
                *start = span.end;
                Some(span)
            })
            .enumerate()
            .filter(|(_, span)| overlaps(&plain.ranges, span))
            .map(|(idx, _)| idx.saturating_sub(context)..(idx + context + 1).min(lines.len())),
    );
    windows
        .into_iter()
        .flatten()
        .map(|idx| ChunkedOutputLineView {
            line: i64::conv(idx),
            content: lines[idx].strip_suffix('\n').unwrap_or(lines[idx]),
        })
        .collect()
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
        let call: AtuinOutputSearchToolCall = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(call.query.as_str(), input["query"].as_str().unwrap());
        assert_eq!(call.limit.get(), limit);
    }

    #[rstest]
    #[case::missing(json!({}))]
    #[case::empty(json!({"query": ""}))]
    #[case::blank(json!({"query": "   "}))]
    #[case::not_a_string(json!({"query": 3}))]
    fn rejects_a_blank_query(#[case] input: serde_json::Value) {
        assert!(serde_json::from_value::<AtuinOutputSearchToolCall>(input).is_err());
    }

    fn plain<'a>(text: &'a str, needle: &str) -> Plain<'a> {
        Plain {
            text: text.into(),
            ranges: text.match_indices(needle).map(|(i, m)| i..i + m.len()).collect(),
        }
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
        "1\terror\n2\tb\n[...skipped 1 lines...]\n4\td\n5\terror\n6\tf"
    )]
    #[case::two_hits_on_one_line_show_it_once(
        "x\nerror error\ny",
        "error",
        "1\tx\n2\terror error\n3\ty"
    )]
    #[case::line_numbers_align("a\nb\nc\nd\ne\nf\ng\nh\ni\nerror", "error", " 9\ti\n10\terror")]
    fn matching_lines_show_hits_with_context(
        #[case] text: &str,
        #[case] needle: &str,
        #[case] expected: &str,
    ) {
        let plain = plain(text, needle);
        let lines = matching_lines(&plain, 1);
        assert_eq!(format_chunked_output_line_views_for_llm(lines.into_iter()), expected);
    }
}
