//! The captured-output tools: `atuin_output` reads one command's output, `atuin_output_search`
//! finds commands by what they printed. Both need the daemon.

use atuin_daemon::grpc::history::pb::ChunkedOutputLineView;

pub mod get;
pub mod search;

/// Advice appended to failures. Hedged on safety: telling the model to re-run unconditionally
/// would invite re-executing destructive commands — the very thing output capture exists to
/// avoid.
const NO_OUTPUT_ADVICE: &str = "If the command is safe and cheap to repeat, re-run it to see its \
                                output; otherwise rely on history metadata (exit code, duration) \
                                instead.";

fn format_line_no(line: i64) -> String {
    if line < 0 {
        line.to_string()
    } else {
        (line + 1).to_string()
    }
}

/// Render `ChunkedOutputLineView`s as `read_file`-style numbered output for the LLM, inserting
/// `[...skipped N lines...]` markers wherever the line numbers jump.
fn format_chunked_output_line_views_for_llm<'a>(
    lines: impl Iterator<Item = ChunkedOutputLineView<'a>> + Clone,
) -> String {
    let width = lines.clone().map(|line| format_line_no(line.line).len()).max();
    let Some(width) = width else {
        return String::new();
    };

    let mut formatted = Vec::new();
    let mut previous_idx = None;
    for line in lines {
        if let Some(previous) = previous_idx {
            if previous >= 0 && line.line < 0 {
                formatted.push("[...skipped an unknown number of lines...]".to_string());
            } else {
                let skipped = line.line.saturating_sub(previous).saturating_sub(1).max(0);
                if skipped > 0 {
                    formatted.push(format!("[...skipped {skipped} lines...]"));
                }
            }
        }
        formatted.push(format!("{:>width$}\t{}", format_line_no(line.line), line.content));
        previous_idx = Some(line.line);
    }
    formatted.join("\n")
}
