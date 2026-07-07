//! Tool lifecycle management within the FSM.
//!
//! Each tool call goes through an independent lifecycle. The ToolManager
//! tracks all tools in the current turn and provides the "all resolved"
//! check that gates turn completion.

use crate::diff::{EditPreview, WritePreview};
use crate::tools::ClientToolCall;

/// Why a tool execution was interrupted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InterruptReason {
    /// User pressed Ctrl+C or Esc during execution.
    User,
    /// The LLM-specified execution timeout expired.
    Timeout(u64),
}

/// Per-tool lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolState {
    /// Permission resolver is running asynchronously.
    CheckingPermission,
    /// Waiting for user to grant/deny via the permission dialog.
    AwaitingPermission,
    /// Actively executing.
    Executing,
    /// Execution completed (result injected into conversation).
    Completed,
    /// User denied permission (error result injected into conversation).
    Denied,
}

/// Cached preview data for rendering tool output.
#[derive(Debug, Clone)]
pub(crate) enum ToolPreviewData {
    /// Shell command VT100 output lines.
    Shell {
        lines: Vec<String>,
        exit_code: Option<i32>,
        interrupted: Option<InterruptReason>,
    },
    /// File edit diff preview.
    Edit(EditPreview),
    /// File write content preview.
    Write(WritePreview),
}

/// A tracked tool call with its current lifecycle state.
#[derive(Debug, Clone)]
pub(crate) struct TrackedTool {
    pub id: String,
    pub tool: ClientToolCall,
    pub state: ToolState,
    /// Cached preview data for rendering (populated during/after execution).
    pub preview: Option<ToolPreviewData>,
    /// Set by the FSM when it emits AbortTool, so that ToolExecutionDone
    /// can distinguish user interrupts from timeouts.
    pub interrupt_reason: Option<InterruptReason>,
}

impl TrackedTool {
    /// Whether this tool has reached a terminal state.
    pub fn is_resolved(&self) -> bool {
        matches!(self.state, ToolState::Completed | ToolState::Denied)
    }

    /// Extract shell preview data (for TurnBuilder compatibility).
    pub fn shell_preview(&self) -> Option<crate::tools::ToolPreview> {
        match &self.preview {
            Some(ToolPreviewData::Shell {
                lines,
                exit_code,
                interrupted,
            }) => Some(crate::tools::ToolPreview {
                lines: lines.clone(),
                exit_code: *exit_code,
                interrupted: interrupted.clone(),
            }),
            _ => None,
        }
    }

    /// Extract edit diff preview (for TurnBuilder compatibility).
    pub fn edit_preview(&self) -> Option<&EditPreview> {
        match &self.preview {
            Some(ToolPreviewData::Edit(p)) => Some(p),
            _ => None,
        }
    }

    /// Extract write content preview (for TurnBuilder compatibility).
    pub fn write_preview(&self) -> Option<&WritePreview> {
        match &self.preview {
            Some(ToolPreviewData::Write(p)) => Some(p),
            _ => None,
        }
    }
}

/// Manages tool call lifecycles for a single turn.
///
/// Tools are inserted when received from the stream and progress through
/// their lifecycle independently. The manager provides aggregate queries
/// (all resolved, any awaiting permission, etc.) that the FSM uses for
/// state transitions.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolManager {
    tools: Vec<TrackedTool>,
}

impl ToolManager {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Insert a new tool in CheckingPermission state.
    pub fn insert(&mut self, id: String, tool: ClientToolCall) {
        self.tools.push(TrackedTool {
            id,
            tool,
            state: ToolState::CheckingPermission,
            preview: None,
            interrupt_reason: None,
        });
    }

    /// Look up a tool by ID.
    pub fn get(&self, id: &str) -> Option<&TrackedTool> {
        self.tools.iter().find(|t| t.id == id)
    }

    /// Look up a tool mutably by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut TrackedTool> {
        self.tools.iter_mut().find(|t| t.id == id)
    }

    /// True if all tools from the given set of IDs have reached a terminal state.
    /// Returns true for an empty set (vacuously — no tools to wait for).
    pub fn all_resolved(&self, tool_ids: &[String]) -> bool {
        tool_ids
            .iter()
            .all(|id| self.get(id).is_some_and(|t| t.is_resolved()))
    }

    /// Find the first tool awaiting user permission.
    pub fn awaiting_permission(&self) -> Option<&TrackedTool> {
        self.tools
            .iter()
            .find(|t| t.state == ToolState::AwaitingPermission)
    }

    /// Get IDs of all non-resolved tools (for cancel).
    pub fn pending_ids(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| !t.is_resolved())
            .map(|t| t.id.clone())
            .collect()
    }

    /// Get IDs of all currently executing tools (for interrupt/abort).
    pub fn executing_ids(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| t.state == ToolState::Executing)
            .map(|t| t.id.clone())
            .collect()
    }

    /// True if any tool has a shell preview with live output.
    pub fn has_executing_preview(&self) -> bool {
        self.tools.iter().any(|t| {
            t.state == ToolState::Executing
                && matches!(t.preview, Some(ToolPreviewData::Shell { .. }))
        })
    }
}
