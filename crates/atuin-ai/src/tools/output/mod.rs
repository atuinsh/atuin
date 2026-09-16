//! The captured-output tools: `atuin_output` reads one command's output, `atuin_output_search`
//! finds commands by what they printed. Both need the daemon.

pub mod get;
pub mod search;

/// Advice appended to failures. Hedged on safety: telling the model to re-run unconditionally
/// would invite re-executing destructive commands — the very thing output capture exists to
/// avoid.
const NO_OUTPUT_ADVICE: &str = "If the command is safe and cheap to repeat, re-run it to see its \
                                output; otherwise rely on history metadata (exit code, duration) \
                                instead.";
