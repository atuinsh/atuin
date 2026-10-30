//! Typed wire protocol for the hook events coding agents send to `atuin hook`.
//!
//! Claude Code, Codex, Copilot and Kiro invoke `atuin hook <agent>` for each tool use and pass
//! the event as JSON on stdin, in Claude Code's shape (Copilot sends it under its `PascalCase`
//! event names), each with its own details: Codex's `tool_response` is a bare string and its
//! exit code is in its transcript; Copilot's `tool_result` holds the output with the exit code
//! at its end, and it names no `tool_use_id`; Kiro's `tool_response` is a bare string ending in
//! `Exit Code: <n>`, its shell tool is `execute_bash`, and it names no `tool_use_id` either.

use std::path::PathBuf;

use atuin_common::string::NonNulStr;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub enum WireToolName {
    /// The tool the agent requested is a shell command: `Bash` in Claude Code, Codex and
    /// Copilot; `execute_bash` in Kiro (`execute_pwsh` on Windows).
    #[serde(alias = "execute_bash", alias = "execute_pwsh")]
    Bash,
    /// Unrecognized wire tool name.
    #[serde(other)]
    Other,
}

/// A hook event exactly as an agent serializes it on stdin.
#[derive(Debug, Deserialize)]
pub struct WireHookEvent {
    /// The lifecycle stage. An unrecognized value decodes to [`HookEventName::Other`].
    pub hook_event_name: HookEventName,
    /// The tool that ran; we only record `Bash`.
    pub tool_name: WireToolName,
    /// Correlates a command's start and end across two `atuin hook` invocations. Copilot and
    /// Kiro send none: their events correlate by session and command instead (see
    /// [`HookEvent`](super::event::HookEvent)).
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The session the command ran in.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The command about to run. Present on `PreToolUse`; absent (and unread)
    /// on the completion events.
    #[serde(default)]
    pub tool_input: Option<WireToolInput>,
    /// How the command finished. Present on `PostToolUse`; absent elsewhere.
    #[serde(default)]
    pub tool_response: Option<WireToolResponse>,
    /// How the command finished, as Copilot reports it on `PostToolUse` (its `tool_result`).
    #[serde(default)]
    pub tool_result: Option<WireToolResult>,
    /// Why the command failed, on Claude Code's `PostToolUseFailure`: for a shell command that
    /// exited non-zero, `Exit code <n>` and then its output.
    #[serde(default)]
    pub error: Option<String>,
    /// The session's transcript, where Codex records a command's exit code (its
    /// `tool_response` is only the output).
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
}

/// The lifecycle stage an event represents.
///
/// The wire values are `PascalCase` and match these variant names exactly.
/// Unrecognized values map to [`HookEventName::Other`] so future or
/// agent-specific events are skipped rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum HookEventName {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    #[serde(other)]
    Other,
}

/// See [`WireHookEvent::tool_input`].
#[derive(Debug, Deserialize)]
pub struct WireToolInput {
    #[serde(default)]
    pub command: Option<NonNulStr>,
    #[serde(default)]
    pub description: Option<String>,
    /// How Copilot runs the command: `async` for one started in the background, whose
    /// `PostToolUse` comes as soon as it has started (`<command started in background with
    /// shellId: 1>`), not when it finishes.
    #[serde(default)]
    pub mode: Option<String>,
    /// The same in Kiro: its `PostToolUse` for a background command comes when it starts.
    #[serde(default)]
    pub run_in_background: Option<bool>,
}

impl WireToolInput {
    /// Whether the command runs on in the background after the agent gets its result.
    pub fn is_background(&self) -> bool {
        self.mode.as_deref() == Some("async") || self.run_in_background == Some(true)
    }
}

/// See [`WireHookEvent::tool_response`].
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum WireToolResponse {
    /// Some harnesses (Codex, Kiro) only return the tool output string. Kiro's ends in the
    /// exit code (`Output:\n<output>\n\n\nExit Code: 0`, Kiro CLI 2.36.0); Codex's is the
    /// output alone.
    Output(String),
    /// Other harnesses return more details.
    /// TODO(markovejnovic): Should we split the enum per-harness?
    Object {
        #[serde(rename = "exitCode", default)]
        exit_code: Option<i64>,
    },
}

impl WireToolResponse {
    /// The exit code the agent reported as a field, if any. A bare response says none here:
    /// see [`Self::kiro_exit_code`].
    pub fn exit_code(&self) -> Option<i64> {
        match self {
            WireToolResponse::Output(_) => None,
            WireToolResponse::Object { exit_code } => *exit_code,
        }
    }

    /// The exit code a Kiro response ends in, its last line `Exit Code: <n>` (`-1` for a call
    /// that never ran: denied, or failed before it was answered). For Kiro's events only: a
    /// Codex response is the output alone, and a command that prints such a line itself must
    /// not pass for it.
    pub fn kiro_exit_code(&self) -> Option<i64> {
        match self {
            WireToolResponse::Output(output) => trailing_exit_code(output),
            WireToolResponse::Object { .. } => None,
        }
    }
}

fn trailing_exit_code(output: &str) -> Option<i64> {
    output.trim_end().lines().next_back()?.strip_prefix("Exit Code:")?.trim().parse().ok()
}

/// See [`WireHookEvent::tool_result`]: what Copilot gave the model, the command's output ending
/// in `<shellId: 0 completed with exit code 1>` (Copilot CLI 1.0.104).
#[derive(Debug, Deserialize)]
pub struct WireToolResult {
    #[serde(default)]
    pub text_result_for_llm: Option<String>,
}

impl WireToolResult {
    /// The exit code the output's trailer reports, if any.
    pub fn exit_code(&self) -> Option<i64> {
        let text = self.text_result_for_llm.as_deref()?;
        let tail = text.trim_end().rsplit('<').next()?;
        let code = tail.strip_suffix('>')?.rsplit("completed with exit code ").next()?;
        code.trim().parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::kiro_success("Output:\n1793383081798933000\n\n\nExit Code: 0", Some(0))]
    #[case::kiro_failure("Output:\nFAILED (failures=1)\n\n\nExit Code: 1", Some(1))]
    #[case::kiro_never_ran(
        "Output:\nThe permission request failed before it was answered, so this call did not run; the user did not reject it.\n\nExit Code: -1",
        Some(-1)
    )]
    #[case::kiro_no_output("Output:\n\n\n\nExit Code: 0\n", Some(0))]
    #[case::output_alone("ls: cannot access '/nope': No such file or directory\n", None)]
    #[case::not_a_number("Output:\nExit Code: lots", None)]
    #[case::empty("", None)]
    fn reads_the_exit_code_at_the_end_of_a_bare_response(
        #[case] output: &str,
        #[case] expected: Option<i64>,
    ) {
        assert_eq!(trailing_exit_code(output), expected);
    }
}
