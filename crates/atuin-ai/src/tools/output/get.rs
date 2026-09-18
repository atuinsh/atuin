//! `atuin_output`: read the captured output of one command by history ID.

use std::path::Path;
use std::str::FromStr;

use atuin_client::history::HistoryId;
use atuin_common::range::PyStyleIdxRange;
use eyre::Result;

use super::{NO_OUTPUT_ADVICE, format_chunked_output_line_views_for_llm};
use crate::permissions::rule::Rule;
use crate::tools::{PermissibleToolCall, ToolOutcome};

#[derive(Debug, Clone)]
pub struct AtuinOutputToolCall {
    pub history_id: HistoryId,
    /// The MCP protocol specifies that ranges should be Python-style, ie. array indices can be
    /// expressed as `[0, -1]` -- where `-1` refers to the element with cursor offset 1 from the end
    /// of the slice.
    pub ranges: Vec<PyStyleIdxRange>,
    /// The command the history entry ran, resolved from the local history
    /// db after parsing (`Effect::ResolveOutputCommand`). Display-only:
    /// `None` until the lookup lands, or when the id isn't known locally.
    pub command: Option<String>,
}

impl TryFrom<&serde_json::Value> for AtuinOutputToolCall {
    type Error = eyre::Error;

    fn try_from(value: &serde_json::Value) -> Result<Self, Self::Error> {
        let history_id: HistoryId = value
            .get("history_id")
            .and_then(|v| v.as_str())
            .and_then(|s| HistoryId::from_str(s).ok())
            .ok_or_else(|| eyre::eyre!("Missing or invalid history ID"))?;

        let ranges =
            value.get("ranges").and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or(&[]);

        let ranges = ranges
            .iter()
            .map(|r| {
                let range = r
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| eyre::eyre!("Each range must be a [start, end] array"))?;

                let start = range[0]
                    .as_i64()
                    .ok_or_else(|| eyre::eyre!("Range start must be an integer"))?;
                let end =
                    range[1].as_i64().ok_or_else(|| eyre::eyre!("Range end must be an integer"))?;

                Ok(PyStyleIdxRange::new(start, end))
            })
            .collect::<Result<Vec<PyStyleIdxRange>, eyre::Error>>()?;

        Ok(Self {
            history_id,
            ranges,
            command: None,
        })
    }
}

impl PermissibleToolCall for AtuinOutputToolCall {
    fn target_dir(&self) -> Option<&Path> {
        None
    }

    fn matches_rule(&self, rule: &Rule) -> bool {
        rule.tool == "AtuinOutput"
    }
}

impl AtuinOutputToolCall {
    pub(crate) async fn execute(&self) -> ToolOutcome {
        let settings = match atuin_client::settings::Settings::new() {
            Ok(settings) => settings,
            Err(e) => return ToolOutcome::Error(format!("Failed to load Atuin settings: {e}")),
        };

        let mut client = match atuin_daemon::HistoryClient::from_settings(&settings).await {
            Ok(client) => client,
            Err(e) => {
                return ToolOutcome::Error(format!(
                    "Captured output is unavailable: could not connect to the Atuin daemon ({e}). \
                     History search still works. {NO_OUTPUT_ADVICE}"
                ));
            }
        };

        let history_id = self.history_id;

        let not_found = || {
            ToolOutcome::Success(format!(
                "No captured output found for history ID {history_id}. Output is only captured \
                 for commands run in an Atuin-enabled terminal while the daemon was running; \
                 older output may also have been dropped. {NO_OUTPUT_ADVICE}"
            ))
        };

        // An empty request means "give me everything": a single `[0, -1]` range spans the whole
        // output.
        let ranges = if self.ranges.is_empty() {
            vec![PyStyleIdxRange::new(0, -1)]
        } else {
            self.ranges.clone()
        };

        let response = match client.get_command_output(history_id, ranges).await {
            Ok(Some(response)) => response,
            Ok(None) => return not_found(),
            Err(e) => return ToolOutcome::Error(format!("Failed to fetch command output: {e}")),
        };

        let body = format_chunked_output_line_views_for_llm(response.lines());
        if body.is_empty() {
            return ToolOutcome::Success(if self.ranges.is_empty() {
                format!("Captured output for history ID {history_id} is empty.")
            } else {
                format!("No lines selected from captured output for history ID {history_id}.")
            });
        }
        let totals = format!("{} bytes, {} lines", response.total_bytes, response.total_lines);
        let meta = response.meta.unwrap_or_default();

        let total_output = if response.truncated {
            format!("{totals} ({} bytes observed before truncation)", meta.output_observed_bytes)
        } else {
            totals
        };

        ToolOutcome::Success(format!(
            "History ID: {history_id}\nTotal output: {total_output}\nSelected output:\n{body}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::history::pb::{
        ChunkedOutputLineView, CommandCapture, CommandCaptureMeta, GetCommandOutputResponse,
    };
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn atuin_output_ranges_are_optional() -> eyre::Result<()> {
        let input = serde_json::json!({
            "history_id": "018f0000000070008000000000000000"
        });

        let call = AtuinOutputToolCall::try_from(&input)?;

        assert_eq!(call.history_id.to_string(), "018f0000000070008000000000000000");
        assert!(call.ranges.is_empty());
        Ok(())
    }

    #[rstest]
    fn atuin_output_parses_line_ranges() -> eyre::Result<()> {
        let input = serde_json::json!({
            "history_id": "018f0000000070008000000000000000",
            "ranges": [[0, 30], [-100, -1]]
        });

        let call = AtuinOutputToolCall::try_from(&input)?;

        assert_eq!(call.ranges, vec![PyStyleIdxRange::new(0, 30), PyStyleIdxRange::new(-100, -1),]);
        Ok(())
    }

    #[rstest]
    fn atuin_output_formats_lines_like_read_file() {
        // 0-based line indices 97 and 99 render as line numbers 98 and 100, with a gap marker.
        let lines = [
            ChunkedOutputLineView {
                line: 97,
                content: "near end",
            },
            ChunkedOutputLineView {
                line: 99,
                content: "end",
            },
        ];

        assert_eq!(
            format_chunked_output_line_views_for_llm(lines.into_iter()),
            " 98\tnear end\n[...skipped 1 lines...]\n100\tend"
        );
    }

    #[rstest]
    fn atuin_output_renders_a_blank_line_instead_of_widening_the_gap() {
        // Line 1 of the output is blank. Selecting it alongside the last two lines must render it
        // as a blank numbered line and report exactly the two lines genuinely left out, "charlie"
        // and "delta". Reconstructing chunk contents with `str::lines` used to swallow the blank
        // line and inflate the marker to three.
        let capture = CommandCapture {
            output_start: "alpha\n\ncharlie\ndelta\necho\nfoxtrot".to_string(),
            output_end: None,
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: 0,
                terminal_width: 80,
                terminal_height: 24,
            }),
        };
        let chunked = GetCommandOutputResponse::build(&capture, &[
            PyStyleIdxRange::new(0, 1),
            PyStyleIdxRange::new(4, 5),
        ]);

        assert_eq!(
            format_chunked_output_line_views_for_llm(chunked.lines()),
            "1\talpha\n2\t\n[...skipped 2 lines...]\n5\techo\n6\tfoxtrot"
        );
    }

    #[rstest]
    fn atuin_output_marks_the_gap_left_by_a_discarded_middle() {
        // The command outran the capture limit, so its middle is gone. The kept tail is numbered
        // from the end, because how many lines went missing -- and so where the tail really
        // starts -- cannot be known. Numbering it from the front instead would repeat line
        // numbers 1..3 and read as one contiguous six-line output.
        let capture = CommandCapture {
            output_start: "alpha\nbravo\ncharlie".to_string(),
            output_end: Some("xray\nyankee\nzulu".to_string()),
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: 1_000_000,
                terminal_width: 80,
                terminal_height: 24,
            }),
        };
        let chunked = GetCommandOutputResponse::build(&capture, &[PyStyleIdxRange::new(0, -1)]);

        assert!(chunked.truncated, "the request spanned the discarded middle");
        assert_eq!(
            format_chunked_output_line_views_for_llm(chunked.lines()),
            " 1\talpha\n 2\tbravo\n 3\tcharlie\n[...skipped an unknown number of \
             lines...]\n-3\txray\n-2\tyankee\n-1\tzulu"
        );
    }

    #[rstest]
    fn atuin_output_numbers_a_tail_only_request_from_the_end() {
        let capture = CommandCapture {
            output_start: "alpha\nbravo\ncharlie".to_string(),
            output_end: Some("xray\nyankee\nzulu".to_string()),
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: 1_000_000,
                terminal_width: 80,
                terminal_height: 24,
            }),
        };
        let chunked = GetCommandOutputResponse::build(&capture, &[PyStyleIdxRange::new(-3, -2)]);

        // Wholly inside the kept tail, so nothing was skipped and nothing is claimed to be.
        assert!(!chunked.truncated);
        assert_eq!(
            format_chunked_output_line_views_for_llm(chunked.lines()),
            "-3\txray\n-2\tyankee"
        );
    }
}
