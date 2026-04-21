//! Events (inputs) to the agent FSM.

use serde_json::Value;

use crate::tools::ToolOutcome;

/// Events that drive state transitions in the agent FSM.
#[derive(Debug, Clone)]
pub(crate) enum Event {
    // ─── User actions ───────────────────────────────────────────
    /// User submitted a message from the input box.
    UserSubmit(String),
    /// User pressed Esc or equivalent cancel action.
    Cancel,
    /// User pressed Enter to execute the suggested command.
    ExecuteCommand,
    /// User pressed Tab to insert the suggested command.
    InsertCommand,
    /// User chose to retry after an error.
    Retry,
    /// User interrupted executing tools (Ctrl+C / Esc during shell execution).
    InterruptTools,

    // ─── Stream lifecycle ───────────────────────────────────────
    /// Stream connection established, first frame received.
    StreamStarted,
    /// Received a chunk of streamed text content.
    StreamChunk(String),
    /// Stream delivered a client-side tool call.
    StreamToolCall {
        id: String,
        name: String,
        input: Value,
    },
    /// Stream delivered a server-side tool result (executed remotely).
    StreamServerToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        remote: bool,
        content_length: Option<usize>,
    },
    /// Stream status changed (e.g. "thinking", "searching").
    StreamStatusChanged(String),
    /// Stream ended normally.
    StreamDone { session_id: String },
    /// Stream encountered an error.
    StreamError(String),

    // ─── Suggest command (terminal tool call) ───────────────────
    /// The suggest_command tool call acts as a stream terminal event.
    /// This is the server signaling "turn complete, here's the command."
    SuggestCommand { id: String, input: Value },

    // ─── Tool lifecycle ─────────────────────────────────────────
    /// Permission resolver completed for a tool.
    PermissionResolved {
        tool_id: String,
        response: PermissionResponse,
    },
    /// User made a permission choice via the dialog.
    PermissionUserChoice {
        tool_id: String,
        choice: PermissionChoice,
    },
    /// Tool execution completed.
    ToolExecutionDone {
        tool_id: String,
        outcome: ToolOutcome,
        /// Preview data computed by the driver (diff, content preview, final shell state).
        preview: Option<super::tools::ToolPreviewData>,
    },
    /// Live preview update for an executing shell command.
    ToolPreviewUpdate {
        tool_id: String,
        lines: Vec<String>,
        exit_code: Option<i32>,
    },

    // ─── Timers ─────────────────────────────────────────────────
    /// Confirmation timeout expired.
    ConfirmationTimeout { timeout_id: u64 },
    /// Shell tool execution timeout expired.
    ToolExecutionTimeout { timeout_id: u64, tool_id: String },

    // ─── Session management ─────────────────────────────────────
    /// User ran /new to start a fresh session.
    NewSession,

    // ─── Slash commands ─────────────────────────────────────────
    /// User submitted a slash command (other than /new).
    /// The driver resolves known commands (like /help) and passes the
    /// rendered content; the FSM just pushes an OOB event.
    SlashCommand { command: String, content: String },
}

/// Result of the permission resolver check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionResponse {
    /// Rule allows this tool call — execute immediately.
    Allowed,
    /// Rule denies this tool call — reject with error.
    Denied,
    /// No matching rule — ask the user.
    Ask,
    /// Session-scoped grant exists — execute immediately (bypass resolver).
    SessionGranted,
}

/// User's choice from the permission dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionChoice {
    /// Allow this one time.
    Allow,
    /// Allow this file for the remainder of the session.
    AllowForSession,
    /// Always allow in this project (writes to project permissions file).
    AlwaysAllowInProject,
    /// Always allow globally (writes to global permissions file, scoped to file).
    AlwaysAllow,
    /// Deny this tool call.
    Deny,
}
