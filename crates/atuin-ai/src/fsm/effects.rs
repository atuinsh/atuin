//! Effects (outputs) from the agent FSM.
//!
//! The FSM returns these as data; the driver is responsible for executing them.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::permissions::rule::Rule;
use crate::permissions::writer::RuleDisposition;
use crate::tools::ClientToolCall;

/// Where to write a permission rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionTarget {
    /// Project-level: `<git_root_or_cwd>/.atuin/permissions.ai.toml`
    Project,
    /// Global: `~/.config/atuin/permissions.ai.toml`
    Global,
}

/// Side effects the driver should execute after a state transition.
#[derive(Debug, Clone)]
pub(crate) enum Effect {
    // ─── Network ────────────────────────────────────────────────
    /// Start a new streaming request to the server.
    StartStream {
        messages: Vec<Value>,
        session_id: Option<String>,
    },
    /// Abort the active stream connection.
    AbortStream,

    // ─── Tool orchestration ─────────────────────────────────────
    /// Run the permission resolver for a tool call.
    CheckPermission {
        tool_id: String,
        tool: ClientToolCall,
    },
    /// Execute a tool (file read, edit, write, shell, history search).
    ExecuteTool {
        tool_id: String,
        tool: ClientToolCall,
    },
    /// Kill a running tool (send interrupt to shell command).
    AbortTool { tool_id: String },

    // ─── Persistence ────────────────────────────────────────────
    /// Persist current conversation state to disk.
    Persist,
    /// Write a permanent permission rule to disk.
    WritePermissionRule {
        target: PermissionTarget,
        rule: Rule,
        disposition: RuleDisposition,
    },
    /// Cache a session-scoped file permission grant.
    CacheSessionGrant { path: PathBuf },
    /// Archive current session and start fresh (IO only — state already updated by FSM).
    ArchiveSession,

    // ─── Timers ─────────────────────────────────────────────────
    /// Schedule a timer that fires an event after the given delay.
    ScheduleTimeout {
        timeout_id: u64,
        duration: Duration,
        kind: TimeoutKind,
    },

    // ─── Exit ───────────────────────────────────────────────────
    /// Exit the application with the given action.
    ExitApp(ExitAction),
}

/// What kind of timeout was scheduled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TimeoutKind {
    /// Dangerous command confirmation dialog auto-dismiss.
    Confirmation,
    /// Shell tool execution timeout — abort the tool if it's still running.
    ToolExecution { tool_id: String },
}

/// What to do when exiting the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitAction {
    /// Run the suggested command.
    Execute(String),
    /// Insert the command into the shell without running.
    Insert(String),
    /// Exit without action.
    Cancel,
}
