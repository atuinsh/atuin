//! Typed wire protocol for the hook events coding agents send to `atuin hook`.
//!
//! Claude Code and Codex invoke `atuin hook <agent>` for each tool use and
//! pass the event as JSON on stdin.

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
    pub command: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// See [`WireHookEvent::tool_response`].
#[derive(Debug, Deserialize)]
pub struct WireToolResponse {
    #[serde(rename = "exitCode", default)]
    pub exit_code: Option<i64>,
}
