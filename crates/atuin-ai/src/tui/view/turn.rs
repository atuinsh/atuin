use std::path::PathBuf;

use crate::fsm::tools::ToolManager;
use crate::tools::descriptor;
use crate::tools::{ClientToolCall, HistorySearchFilterMode, ToolPreview};
use crate::tui::ConversationEvent;

/// Server-sent danger level for a suggested command
#[derive(Debug)]
pub(crate) enum DangerLevel {
    Low(Option<String>),
    Medium(Option<String>),
    High(Option<String>),
    Unknown(Option<String>),
}

impl DangerLevel {
    pub(crate) fn notes(&self) -> Option<&String> {
        match self {
            DangerLevel::Low(notes) => notes.as_ref(),
            DangerLevel::Medium(notes) => notes.as_ref(),
            DangerLevel::High(notes) => notes.as_ref(),
            DangerLevel::Unknown(notes) => notes.as_ref(),
        }
    }
}

impl From<(&String, &String)> for DangerLevel {
    fn from((danger_level, danger_notes): (&String, &String)) -> Self {
        let notes = if danger_notes.is_empty() {
            None
        } else {
            Some(danger_notes.to_string())
        };

        match danger_level.as_str() {
            "low" => DangerLevel::Low(notes),
            "medium" => DangerLevel::Medium(notes),
            "med" => DangerLevel::Medium(notes),
            "high" => DangerLevel::High(notes),
            _ => DangerLevel::Unknown(notes),
        }
    }
}

/// Server-sent confidence level for a suggested command
#[derive(Debug)]
pub(crate) enum ConfidenceLevel {
    Low(Option<String>),
    Medium(Option<String>),
    High(Option<String>),
    Unknown(Option<String>),
}

impl ConfidenceLevel {
    pub(crate) fn notes(&self) -> Option<&String> {
        match self {
            ConfidenceLevel::Low(notes) => notes.as_ref(),
            ConfidenceLevel::Medium(notes) => notes.as_ref(),
            ConfidenceLevel::High(notes) => notes.as_ref(),
            ConfidenceLevel::Unknown(notes) => notes.as_ref(),
        }
    }
}

impl From<(&String, &String)> for ConfidenceLevel {
    fn from((confidence_level, confidence_notes): (&String, &String)) -> Self {
        let notes = if confidence_notes.is_empty() {
            None
        } else {
            Some(confidence_notes.to_string())
        };

        match confidence_level.as_str() {
            "low" => ConfidenceLevel::Low(notes),
            "medium" => ConfidenceLevel::Medium(notes),
            "med" => ConfidenceLevel::Medium(notes),
            "high" => ConfidenceLevel::High(notes),
            _ => ConfidenceLevel::Unknown(notes),
        }
    }
}

#[derive(Debug)]
pub(crate) enum UiEvent {
    Text {
        content: String,
    },
    ToolCall(ToolCallDetails),
    /// Consecutive client-side tool calls of the same groupable kind, collapsed
    /// into one unit so the view can render a shared status line + a list of
    /// individual entries.
    ToolGroup(ToolGroup),
    ToolSummary(ToolSummary),
    SuggestedCommand(SuggestedCommandDetails),
    OutOfBandOutput(OutOfBandOutputDetails),
}

/// A run of consecutive client-side tool calls of the same groupable kind.
#[derive(Debug)]
pub(crate) struct ToolGroup {
    pub(crate) kind: ToolGroupKind,
    pub(crate) calls: Vec<ToolCallDetails>,
}

impl ToolGroup {
    /// True if any call in the group is still pending.
    pub(crate) fn any_pending(&self) -> bool {
        self.calls
            .iter()
            .any(|c| c.status == ToolResultStatus::Pending)
    }
}

/// Which kind of client-side tools this group holds.
///
/// Only tool types that benefit from grouped presentation appear here.
/// Shell (needs its own viewport) and FileWrite (wants diffs/contents) are
/// intentionally absent — those render as individual `UiEvent::ToolCall`s.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum ToolGroupKind {
    FileRead,
    HistorySearch,
}

/// Tool-type-specific data for rendering in the view layer.
///
/// Each variant carries the data a per-tool renderer component needs.
/// Built by TurnBuilder from ToolTracker + ConversationEvent data.
#[derive(Debug)]
pub(crate) enum ToolRenderData {
    /// Shell command with live/cached VT100 output preview.
    Shell {
        command: String,
        preview: Option<ToolPreview>,
    },
    /// File read operation.
    FileRead { path: PathBuf },
    /// File edit (str_replace) operation.
    FileEdit {
        path: PathBuf,
        preview: Option<crate::diff::EditPreview>,
    },
    /// File write/create operation.
    FileWrite {
        path: PathBuf,
        preview: Option<crate::diff::WritePreview>,
    },
    /// Atuin history search.
    HistorySearch {
        query: String,
        filter_modes: Vec<HistorySearchFilterMode>,
    },
    /// Server-side tool — no client rendering data available.
    Remote,
}

impl ToolRenderData {
    pub(crate) fn is_remote(&self) -> bool {
        matches!(self, ToolRenderData::Remote)
    }

    /// The group kind this tool should collapse into, if any.
    ///
    /// Returns `None` for tools that render as individual `UiEvent::ToolCall`s
    /// (shell, file writes, remote).
    pub(crate) fn group_kind(&self) -> Option<ToolGroupKind> {
        match self {
            ToolRenderData::FileRead { .. } => Some(ToolGroupKind::FileRead),
            ToolRenderData::HistorySearch { .. } => Some(ToolGroupKind::HistorySearch),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ToolCallDetails {
    pub(crate) tool_use_id: String,
    pub(crate) name: String,
    pub(crate) status: ToolResultStatus,
    pub(crate) render_data: ToolRenderData,
}

#[derive(Debug)]
pub(crate) struct SuggestedCommandDetails {
    pub(crate) command: String,
    pub(crate) danger_level: DangerLevel,
    pub(crate) confidence_level: ConfidenceLevel,
}

#[derive(Debug)]
pub(crate) struct OutOfBandOutputDetails {
    pub(crate) command: Option<String>,
    pub(crate) content: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ToolResultStatus {
    Pending,
    Success,
    Error,
}

#[derive(Debug)]
pub(crate) enum UiTurn {
    User { events: Vec<UiEvent> },
    Agent { events: Vec<UiEvent> },
    OutOfBand { events: Vec<UiEvent> },
}

pub(crate) struct TurnBuilder<'a> {
    turns: Vec<UiTurn>,
    current_turn: Option<UiTurn>,
    tracker: &'a ToolManager,
}

/// A struct to iteratively build [UiTurn] events from [ConversationEvent]s.
impl<'a> TurnBuilder<'a> {
    pub(crate) fn new(tracker: &'a ToolManager) -> Self {
        Self {
            turns: Vec::new(),
            current_turn: None,
            tracker,
        }
    }

    pub(crate) fn add_event(&mut self, event: &ConversationEvent) {
        match event {
            ConversationEvent::UserMessage { content } => {
                self.add_user_message(content);
            }
            ConversationEvent::Text { content } => {
                self.add_agent_text(content);
            }
            ConversationEvent::ToolCall { id, name, input } => {
                if name == "suggest_command" {
                    self.add_suggested_command(input);
                } else {
                    self.add_tool_call(id, name, input);
                }
            }
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                self.add_tool_result(tool_use_id, content, *is_error);
            }
            ConversationEvent::OutOfBandOutput {
                name,
                command,
                content,
            } => {
                self.add_out_of_band_output(name, command.as_deref(), content);
            }
            ConversationEvent::SystemContext { .. } => {
                // Not rendered in the TUI — only sent to the API
            }
        }
    }

    pub(crate) fn build(&mut self) -> Vec<UiTurn> {
        self.commit_turn();

        // Within each agent turn:
        // - Consecutive remote tool calls collapse into a ToolSummary
        // - Consecutive client-side tool calls of the same group kind collapse
        //   into a ToolGroup (e.g. N file reads → one group)
        // - All other events pass through unchanged
        for turn in &mut self.turns {
            if let UiTurn::Agent { events } = turn {
                let mut new_events: Vec<UiEvent> = Vec::new();
                let mut pending_remote: Vec<ToolCallDetails> = Vec::new();
                let mut pending_group: Option<(ToolGroupKind, Vec<ToolCallDetails>)> = None;

                for event in events.drain(..) {
                    match event {
                        UiEvent::ToolCall(details) if details.render_data.is_remote() => {
                            flush_group(&mut pending_group, &mut new_events);
                            pending_remote.push(details);
                        }
                        UiEvent::ToolCall(details)
                            if details.render_data.group_kind().is_some() =>
                        {
                            flush_remote(&mut pending_remote, &mut new_events);

                            let kind = details.render_data.group_kind().unwrap();
                            match pending_group.as_mut() {
                                Some((current_kind, calls)) if *current_kind == kind => {
                                    calls.push(details);
                                }
                                _ => {
                                    flush_group(&mut pending_group, &mut new_events);
                                    pending_group = Some((kind, vec![details]));
                                }
                            }
                        }
                        other => {
                            flush_remote(&mut pending_remote, &mut new_events);
                            flush_group(&mut pending_group, &mut new_events);
                            new_events.push(other);
                        }
                    }
                }

                flush_remote(&mut pending_remote, &mut new_events);
                flush_group(&mut pending_group, &mut new_events);

                *events = new_events;
            }
        }

        std::mem::take(&mut self.turns)
    }

    fn commit_turn(&mut self) {
        if let Some(turn) = self.current_turn.take() {
            self.turns.push(turn);
        }
    }

    fn start_user_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::User { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::User { events: vec![] });
        }
    }

    fn start_agent_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::Agent { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::Agent { events: vec![] });
        }
    }

    fn start_out_of_band_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::OutOfBand { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::OutOfBand { events: vec![] });
        }
    }

    fn turn_mut_unsafe(&mut self) -> &mut UiTurn {
        self.current_turn.as_mut().unwrap()
    }

    fn add_user_message(&mut self, content: &str) {
        self.start_user_turn();
        if let UiTurn::User { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_agent_text(&mut self, content: &str) {
        if content.trim().is_empty() {
            return;
        }
        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_suggested_command(&mut self, input: &serde_json::Value) {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if command.is_empty() {
            return;
        }

        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            let danger_level = input
                .get("danger")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let confidence_level = input
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let danger_notes = input
                .get("danger_notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let confidence_notes = input
                .get("confidence_notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let danger = DangerLevel::from((&danger_level, &danger_notes));
            let confidence = ConfidenceLevel::from((&confidence_level, &confidence_notes));

            events.push(UiEvent::SuggestedCommand(SuggestedCommandDetails {
                command: input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                danger_level: danger,
                confidence_level: confidence,
            }));
        }
    }

    fn add_tool_call(&mut self, id: &str, name: &str, _input: &serde_json::Value) {
        let render_data = self.build_render_data(id, name);

        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::ToolCall(ToolCallDetails {
                tool_use_id: id.to_string(),
                name: name.to_string(),
                status: ToolResultStatus::Pending,
                render_data,
            }));
        }
    }

    /// Build tool-type-specific render data from the ToolTracker.
    ///
    /// For client-side tools, the tracker holds the typed `ClientToolCall` and
    /// any live/cached preview data. For server-side (or unknown) tools, we
    /// fall back to `ToolRenderData::Remote`.
    fn build_render_data(&self, id: &str, _name: &str) -> ToolRenderData {
        if let Some(tracked) = self.tracker.get(id) {
            match &tracked.tool {
                ClientToolCall::Shell(shell) => ToolRenderData::Shell {
                    command: shell.command.clone(),
                    preview: tracked.shell_preview(),
                },
                ClientToolCall::Read(read) => ToolRenderData::FileRead {
                    path: read.path.clone(),
                },
                ClientToolCall::Edit(edit) => ToolRenderData::FileEdit {
                    path: edit.path.clone(),
                    preview: tracked.edit_preview().cloned(),
                },
                ClientToolCall::Write(write) => ToolRenderData::FileWrite {
                    path: write.path.clone(),
                    preview: tracked.write_preview().cloned(),
                },
                ClientToolCall::AtuinHistory(history) => ToolRenderData::HistorySearch {
                    query: history.query.clone(),
                    filter_modes: history.filter_modes.clone(),
                },
            }
        } else {
            // Not in tracker → server-side tool
            ToolRenderData::Remote
        }
    }

    fn add_tool_result(&mut self, tool_use_id: &str, _content: &str, is_error: bool) {
        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            let event = events.iter_mut().find(|e| match e {
                UiEvent::ToolCall(ToolCallDetails {
                    tool_use_id: id, ..
                }) => id == tool_use_id,
                _ => false,
            });
            if let Some(UiEvent::ToolCall(ToolCallDetails { status, .. })) = event {
                *status = if is_error {
                    ToolResultStatus::Error
                } else {
                    ToolResultStatus::Success
                };
            }
        }
    }

    fn add_out_of_band_output(&mut self, _name: &str, command: Option<&str>, content: &str) {
        self.start_out_of_band_turn();
        if let UiTurn::OutOfBand { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::OutOfBandOutput(OutOfBandOutputDetails {
                command: command.map(|c| c.to_string()),
                content: content.to_string(),
            }));
        }
    }
}

/// Drain pending remote tool calls into a `ToolSummary`.
fn flush_remote(pending: &mut Vec<ToolCallDetails>, out: &mut Vec<UiEvent>) {
    if !pending.is_empty() {
        out.push(UiEvent::ToolSummary(ToolSummary {
            tool_calls: std::mem::take(pending),
        }));
    }
}

/// Drain a pending client-side tool group into a `ToolGroup`.
fn flush_group(
    pending: &mut Option<(ToolGroupKind, Vec<ToolCallDetails>)>,
    out: &mut Vec<UiEvent>,
) {
    if let Some((kind, calls)) = pending.take() {
        out.push(UiEvent::ToolGroup(ToolGroup { kind, calls }));
    }
}

#[derive(Debug)]
pub(crate) struct ToolSummary {
    tool_calls: Vec<ToolCallDetails>,
}

impl ToolSummary {
    /// Determines the summary line:
    /// - If any call is pending, use present tense verb with `-ing`
    /// - If multiple calls are complete, say "Used n tools"
    /// - If a single call is complete, use past tense verb
    pub(crate) fn summary(&self) -> String {
        if self.any_pending() {
            // Find the last pending tool for the active verb
            if let Some(pending) = self
                .tool_calls
                .iter()
                .rev()
                .find(|t| t.status == ToolResultStatus::Pending)
            {
                return Self::progressive_verb(&pending.name);
            }
        }

        if self.tool_calls.len() == 1 {
            return Self::past_verb(&self.tool_calls[0].name);
        }

        format!("Used {} tools", self.tool_calls.len())
    }

    /// Determines if the spinner should be spinning
    pub(crate) fn any_pending(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|tool_call| tool_call.status == ToolResultStatus::Pending)
    }

    /// Present-tense progressive verb for a tool name (e.g. "Searching...")
    fn progressive_verb(name: &str) -> String {
        descriptor::by_name(name)
            .map(|d| d.progressive_verb.to_string())
            .unwrap_or_else(|| format!("Running {}...", name.replace('_', " ")))
    }

    /// Past-tense verb for a tool name (e.g. "Searched")
    fn past_verb(name: &str) -> String {
        descriptor::by_name(name)
            .map(|d| d.past_verb.to_string())
            .unwrap_or_else(|| format!("Ran {}", name.replace('_', " ")))
    }
}
