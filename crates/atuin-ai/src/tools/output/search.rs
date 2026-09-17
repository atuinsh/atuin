//! `atuin_output_search`: full-text search over captured command output.

use atuin_client::database::Sqlite;
use atuin_client::settings::{OutputCapture, Settings};
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::client::SearchClient;
use atuin_daemon::grpc::history::pb::ChunkedOutputLineView;
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{NO_OUTPUT_ADVICE, format_chunked_output_line_views_for_llm};
use crate::history_format::format_history_search_result;
use crate::tools::ToolOutcome;

/// Output lines shown on each side of a matching line.
const CONTEXT_LINES: u32 = 1;

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
                .search_command_output(self.query.to_string(), None, Some(CONTEXT_LINES))
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
                let plain: Vec<_> =
                    m.lines.iter().map(|l| (l.line, l.content.to_plain().text)).collect();
                let views = plain.iter().map(|(line, text)| ChunkedOutputLineView {
                    line: *line,
                    content: text,
                });
                format!(
                    "{}Matching output lines:\n{}\n",
                    format_history_search_result(i + 1, history, local_offset),
                    format_chunked_output_line_views_for_llm(views),
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
}
