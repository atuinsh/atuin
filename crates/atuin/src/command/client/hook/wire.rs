//! Typed wire protocol for the hook events coding agents send to `atuin hook`.
//!
//! Claude Code and Codex invoke `atuin hook <agent>` for each tool use and
//! pass the event as JSON on stdin.

use atuin_common::string::NonNulStr;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub enum WireToolName {
    /// The tool the agent requested is a Bash command.
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
    /// Correlates a command's start and end across two `atuin hook` invocations.
    pub tool_use_id: String,
    /// The command about to run. Present on `PreToolUse`; absent (and unread)
    /// on the completion events.
    #[serde(default)]
    pub tool_input: Option<WireToolInput>,
    /// How the command finished. Present on `PostToolUse`; absent elsewhere.
    #[serde(default)]
    pub tool_response: Option<WireToolResponse>,
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
}

/// See [`WireHookEvent::tool_response`].
#[derive(Debug, Deserialize)]
pub struct WireToolResponse {
    #[serde(rename = "exitCode", default)]
    pub exit_code: Option<i64>,
}

/// A hook event as Antigravity (`agy`) serializes it on stdin.
///
/// Same lifecycle vocabulary as Claude Code (`PreToolUse`/`PostToolUse`) but a
/// different shape: camelCase keys, the tool call nested under `toolCall`, the
/// command under `toolCall.args.CommandLine`, and no tool-use id — a start is
/// correlated with its end through `conversationId` plus `stepIdx`. Completion
/// carries no numeric exit code, only an `error` string that is empty on
/// success. See <https://antigravity.google/docs/hooks/>.
#[derive(Debug, Deserialize)]
pub struct AgyHookEvent {
    /// The lifecycle stage. An unrecognized value decodes to [`AgyEventName::Other`].
    #[serde(rename = "hookEventName")]
    pub event_name: AgyEventName,
    /// The proposed or executed tool call. Present on `PreToolUse`/`PostToolUse`.
    #[serde(rename = "toolCall", default)]
    pub tool_call: Option<AgyToolCall>,
    /// Correlates a command's start and end across two hook invocations,
    /// together with [`AgyHookEvent::step_idx`].
    #[serde(rename = "conversationId", default)]
    pub conversation_id: Option<String>,
    /// The 0-based index of the current step in the trajectory.
    #[serde(rename = "stepIdx", default)]
    pub step_idx: Option<i64>,
    /// Detailed runtime error message when the tool call failed. Empty or
    /// absent on success. Present on `PostToolUse`; absent elsewhere.
    #[serde(default)]
    pub error: Option<String>,
}

/// The lifecycle stage an Antigravity event represents.
///
/// The wire values are `PascalCase` and match these variant names exactly.
/// Unrecognized values map to [`AgyEventName::Other`] so future or
/// agent-specific events are skipped rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum AgyEventName {
    PreToolUse,
    PostToolUse,
    #[serde(other)]
    Other,
}

/// See [`AgyHookEvent::tool_call`].
#[derive(Debug, Deserialize)]
pub struct AgyToolCall {
    /// The tool being executed. We only record `run_command`.
    #[serde(default)]
    pub name: Option<String>,
    /// The arguments passed to the tool.
    #[serde(default)]
    pub args: Option<AgyToolArgs>,
}

/// See [`AgyToolCall::args`].
#[derive(Debug, Deserialize)]
pub struct AgyToolArgs {
    /// The shell command line. Present on `PreToolUse` for `run_command`.
    #[serde(rename = "CommandLine", default)]
    pub command_line: Option<NonNulStr>,
}
