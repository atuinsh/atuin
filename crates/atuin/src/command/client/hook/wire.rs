//! Typed wire protocol for the hook events coding agents send to `atuin hook`.
//!
//! Claude Code, Codex, Copilot and Kiro invoke `atuin hook <agent>` for each tool use and pass
//! the event as JSON on stdin, in Claude Code's shape (Copilot sends it under its `PascalCase`
//! event names), each with its own details: Codex's `tool_response` is a bare string and its
//! exit code is in its transcript; Copilot's `tool_result` holds the output with the exit code
//! at its end, and it names no `tool_use_id`; Kiro's `tool_response` is a bare string ending in
//! `Exit Code: <n>`, its shell tool is `execute_bash`, and it names no `tool_use_id` either.
//!
//! Hermes Agent sends a shape of its own, through atuin's Hermes plugin: see [`WireHermesEvent`].

use std::path::PathBuf;

use atuin_common::string::NonNulStr;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
pub enum WireToolName {
    /// The tool the agent requested is a shell command: `Bash` in Claude Code, Codex and
    /// Copilot; `run_terminal_command` in Grok Build; `execute_bash` in Kiro (`execute_pwsh` on
    /// Windows).
    #[serde(alias = "run_terminal_command", alias = "execute_bash", alias = "execute_pwsh")]
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
        #[serde(rename = "exitCode", alias = "exit_code", default)]
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

/// A Hermes Agent tool call, as atuin's Hermes plugin sends it: the keyword arguments Hermes
/// passes its `pre_tool_call` and `post_tool_call` hooks (v0.23.0), plus the `hook_event_name`
/// and the `cwd` and `called_at` the plugin adds. Fields atuin doesn't read (`task_id`,
/// `turn_id`, `status`, ...) are ignored.
#[derive(Debug, Deserialize)]
pub struct WireHermesEvent {
    pub hook_event_name: HermesEventName,
    pub tool_name: HermesToolName,
    /// Correlates a command's start and end. Some providers repeat ids (llama.cpp sends one
    /// constant id), so it is only unique within a session.
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub args: Option<WireHermesArgs>,
    /// What the tool returned, on `post_tool_call`: for `terminal`, a JSON object in a string
    /// (see [`WireHermesTerminalResult`]).
    #[serde(default)]
    pub result: Option<String>,
    /// Where the command runs. Hermes's hooks run in its gateway, whose directory is not the
    /// command's, so the plugin looks it up.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// When Hermes called the hook, in Unix seconds. The plugin queues its events and hands them
    /// to atuin one at a time, so the event can arrive well after its command started.
    #[serde(default)]
    pub called_at: Option<f64>,
    /// How long the tool call took, as Hermes measured it, on `post_tool_call`.
    #[serde(default)]
    pub duration_ms: Option<f64>,
}

impl WireHermesEvent {
    /// [`Self::called_at`] as a time, `None` when absent or out of range.
    pub fn called_at(&self) -> Option<OffsetDateTime> {
        let seconds = self.called_at.filter(|s| s.is_finite() && *s >= 0.0)?;
        #[allow(clippy::cast_possible_truncation)]
        let nanos = (seconds * 1e9) as i128;
        OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()
    }

    /// [`Self::duration_ms`] in nanoseconds, as history keeps durations.
    pub fn duration_nanos(&self) -> Option<u64> {
        let ms = self.duration_ms.filter(|ms| ms.is_finite() && *ms >= 0.0)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some((ms * 1e6) as u64)
    }
}

/// See [`WireHermesEvent::hook_event_name`]. Unrecognized values map to
/// [`HermesEventName::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HermesEventName {
    PreToolCall,
    PostToolCall,
    #[serde(other)]
    Other,
}

/// See [`WireHermesEvent::tool_name`]; we only record `terminal`, Hermes's shell tool.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HermesToolName {
    Terminal,
    #[serde(other)]
    Other,
}

/// The `terminal` tool's arguments (`tools/terminal_tool.py`). Those of other tools decode too,
/// as nothing.
#[derive(Debug, Deserialize)]
pub struct WireHermesArgs {
    #[serde(default)]
    pub command: Option<NonNulStr>,
    /// Whether the command runs on in the background, its end coming as soon as it has started.
    /// Any JSON, since a model may send the string `"true"`.
    #[serde(default)]
    pub background: Option<Value>,
}

impl WireHermesArgs {
    pub fn is_background(&self) -> bool {
        match &self.background {
            Some(Value::Bool(background)) => *background,
            Some(Value::String(background)) => background.eq_ignore_ascii_case("true"),
            _ => false,
        }
    }
}

/// What the `terminal` tool returns (`tools/terminal_tool.py`): `{"output", "exit_code",
/// "error"}`, with a `status` for a command that was `blocked`, is `pending_approval`, or was
/// `yielded_to_background`.
#[derive(Debug, Deserialize)]
pub struct WireHermesTerminalResult {
    /// `None` when the result has no `exit_code`, `Some(Value::Null)` when it is `null`.
    #[serde(default, deserialize_with = "present")]
    pub exit_code: Option<Value>,
    #[serde(default)]
    pub status: Option<String>,
}

/// A field that is there, `null` included.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// How a Hermes `terminal` call ended, as far as its result says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HermesTerminalOutcome {
    /// It exited with this code. A command Hermes refused to run (`blocked`,
    /// `pending_approval`) says `-1`.
    Exited(i64),
    /// It runs on in the background, so its exit is not known: Hermes moved it there after it
    /// overran (`yielded_to_background`, `exit_code: null`).
    Running,
    /// It never ran: the call failed before it got that far, with no exit code (Hermes's
    /// argument checks answer `{"error": ...}` alone).
    NeverRan,
}

impl HermesTerminalOutcome {
    /// The outcome `result` (a [`WireHermesEvent::result`]) reports.
    pub fn of(result: Option<&str>) -> Self {
        let Some(result) =
            result.and_then(|result| serde_json::from_str::<WireHermesTerminalResult>(result).ok())
        else {
            return Self::NeverRan;
        };
        if result.status.as_deref() == Some("yielded_to_background") {
            return Self::Running;
        }
        match result.exit_code {
            Some(Value::Null) => Self::Running,
            Some(code) => code.as_i64().map_or(Self::NeverRan, Self::Exited),
            None => Self::NeverRan,
        }
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

    #[rstest]
    #[case::exited(
        r#"{"output": "hi", "exit_code": 0, "error": null}"#,
        HermesTerminalOutcome::Exited(0)
    )]
    #[case::failed(
        r#"{"output": "", "exit_code": 1, "error": null}"#,
        HermesTerminalOutcome::Exited(1)
    )]
    #[case::timed_out(
        r#"{"output": "", "exit_code": 124, "error": "Command timed out after 180 seconds"}"#,
        HermesTerminalOutcome::Exited(124)
    )]
    #[case::blocked(
        r#"{"output": "", "exit_code": -1, "error": "Command denied", "status": "blocked"}"#,
        HermesTerminalOutcome::Exited(-1)
    )]
    #[case::yielded(
        r#"{"output": "", "exit_code": null, "error": null, "status": "yielded_to_background", "session_id": "proc_1"}"#,
        HermesTerminalOutcome::Running
    )]
    #[case::null_exit(
        r#"{"output": "", "exit_code": null, "error": null}"#,
        HermesTerminalOutcome::Running
    )]
    #[case::rejected_arguments(
        r#"{"error": "command is required"}"#,
        HermesTerminalOutcome::NeverRan
    )]
    #[case::not_a_number(r#"{"exit_code": "zero"}"#, HermesTerminalOutcome::NeverRan)]
    #[case::not_json("Command failed", HermesTerminalOutcome::NeverRan)]
    fn reads_how_a_hermes_command_ended(
        #[case] result: &str,
        #[case] expected: HermesTerminalOutcome,
    ) {
        assert_eq!(HermesTerminalOutcome::of(Some(result)), expected);
    }

    #[rstest]
    fn a_hermes_end_without_a_result_never_ran() {
        assert_eq!(HermesTerminalOutcome::of(None), HermesTerminalOutcome::NeverRan);
    }
}
