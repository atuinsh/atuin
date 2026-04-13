//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. Conversation events match the API protocol format.

use tokio::task::AbortHandle;

use crate::tools::{ClientToolCall, ToolOutcome, ToolTracker};

/// Streaming status indicators from server
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamingStatus {
    Processing,
    Searching,
    Thinking,
    WaitingForTools,
}

impl StreamingStatus {
    pub(crate) fn from_status_str(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "searching" => Self::Searching,
            "waiting_for_tools" => Self::WaitingForTools,
            _ => Self::Thinking,
        }
    }
}

/// Conversation event types matching the API protocol
#[derive(Debug, Clone)]
pub(crate) enum ConversationEvent {
    /// User message (what the user typed)
    UserMessage { content: String },
    /// Text content from assistant (streamed or complete)
    Text { content: String },
    /// Tool call from assistant
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (from server-side or client-side execution)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        /// Server-side results are stored in the DB; the client sends an opaque
        /// reference (`remote: true`) instead of the full content.
        remote: bool,
        /// Approximate content length for token estimation of remote results.
        content_length: Option<usize>,
    },
    /// Out-of-band output from the system - not sent to the server
    OutOfBandOutput {
        name: String,
        command: Option<String>,
        content: String,
    },
}

impl ConversationEvent {
    /// Extract command from a suggest_command tool call
    pub(crate) fn as_command(&self) -> Option<&str> {
        if let ConversationEvent::ToolCall { name, input, .. } = self
            && name == "suggest_command"
        {
            return input.get("command").and_then(|v| v.as_str());
        }
        None
    }
}

/// Application mode for key handling and footer text.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum AppMode {
    /// User is typing input
    Input,
    /// Waiting for generation (showing spinner)
    Generating,
    /// Streaming SSE response
    Streaming,
    /// Error state, can retry
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitAction {
    /// Run the command
    Execute(String),
    /// Insert command without running
    Insert(String),
    /// User canceled
    Cancel,
}

/// Owned event log and session ID
#[derive(Debug)]
pub(crate) struct Conversation {
    /// Conversation events (source of truth, matches API protocol)
    pub events: Vec<ConversationEvent>,
    /// Session ID from server
    pub session_id: Option<String>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            session_id: None,
        }
    }

    /// Convert conversation events to Claude API message format
    pub fn events_to_messages(&self) -> Vec<serde_json::Value> {
        let mut messages = Vec::new();
        let mut i = 0;
        let events = &self.events;

        while i < events.len() {
            match &events[i] {
                ConversationEvent::UserMessage { content } => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                    i += 1;
                }
                ConversationEvent::Text { content } => {
                    // Check if the next event(s) are ToolCalls — if so, combine
                    // into a single assistant message with mixed content blocks.
                    let next_is_tool_call = events
                        .get(i + 1)
                        .is_some_and(|e| matches!(e, ConversationEvent::ToolCall { .. }));

                    if next_is_tool_call {
                        let mut content_blocks = Vec::new();

                        if !content.is_empty() {
                            content_blocks.push(serde_json::json!({
                                "type": "text",
                                "text": content
                            }));
                        }

                        while let Some(ConversationEvent::ToolCall {
                            id, name, input, ..
                        }) = events.get(i + 1)
                        {
                            content_blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input
                            }));
                            i += 1;
                        }

                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content_blocks
                        }));
                        i += 1;
                    } else {
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content
                        }));
                        i += 1;
                    }
                }
                ConversationEvent::ToolCall { .. } => {
                    // ToolCalls without preceding Text (shouldn't normally happen,
                    // but handle defensively)
                    let mut tool_uses = Vec::new();
                    while i < events.len() {
                        if let ConversationEvent::ToolCall {
                            id, name, input, ..
                        } = &events[i]
                        {
                            tool_uses.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input
                            }));
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": tool_uses
                    }));
                }
                ConversationEvent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    remote,
                    content_length,
                } => {
                    let tool_result = if *remote {
                        let mut obj = serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "remote": true,
                            "is_error": is_error
                        });
                        if let Some(len) = content_length {
                            obj["content_length"] = serde_json::json!(len);
                        }
                        obj
                    } else {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error
                        })
                    };
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [tool_result]
                    }));
                    i += 1;
                }
                ConversationEvent::OutOfBandOutput { .. } => {
                    // Out-of-band output is not sent to the server, so we don't need to add it to the messages
                    i += 1;
                }
            }
        }

        messages
    }

    /// Get the most recent command from events
    pub fn current_command(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| e.as_command())
    }

    /// Check if any turn in the conversation has a command
    pub fn has_any_command(&self) -> bool {
        self.events.iter().any(|e| {
            if let ConversationEvent::ToolCall { name, input, .. } = e {
                name == "suggest_command" && input.get("command").and_then(|v| v.as_str()).is_some()
            } else {
                false
            }
        })
    }

    /// Check if the most recent command is marked dangerous
    pub fn is_current_command_dangerous(&self) -> bool {
        self.events
            .iter()
            .rev()
            .find_map(|e| {
                if let ConversationEvent::ToolCall { name, input, .. } = e
                    && name == "suggest_command"
                {
                    let danger_level = input
                        .get("danger")
                        .and_then(|v| v.as_str())
                        .unwrap_or("low");
                    return Some(
                        danger_level == "high" || danger_level == "medium" || danger_level == "med",
                    );
                }
                None
            })
            .unwrap_or(false)
    }

    /// Get a mutable reference to the last Text event's content (the streaming buffer).
    fn streaming_content_mut(&mut self) -> Option<&mut String> {
        self.events.iter_mut().rev().find_map(|e| {
            if let ConversationEvent::Text { content } = e {
                Some(content)
            } else {
                None
            }
        })
    }

    /// Remove trailing empty Text events from the events list
    fn remove_empty_trailing_text(&mut self) {
        while let Some(ConversationEvent::Text { content }) = self.events.last() {
            if content.is_empty() {
                self.events.pop();
            } else {
                break;
            }
        }
    }

    /// Append text chunk during streaming (mutates the last Text event in-place)
    pub fn append_streaming_text(&mut self, chunk: &str) {
        // If the last event isn't a Text, we need a fresh buffer
        // (e.g. after a tool call removed the empty streaming buffer)
        if !matches!(self.events.last(), Some(ConversationEvent::Text { .. })) {
            self.events.push(ConversationEvent::Text {
                content: String::new(),
            });
        }

        if let Some(content) = self.streaming_content_mut() {
            if content.is_empty() {
                // First chunk(s): trim leading whitespace
                let trimmed = chunk.trim_start();
                if !trimmed.is_empty() {
                    content.push_str(trimmed);
                }
            } else {
                content.push_str(chunk);
            }
        }
    }

    /// Add a tool result event during streaming
    pub fn add_tool_result(
        &mut self,
        tool_use_id: String,
        content: String,
        is_error: bool,
        remote: bool,
        content_length: Option<usize>,
    ) {
        self.events.push(ConversationEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
            remote,
            content_length,
        });
    }

    /// Store session ID from server response
    pub fn store_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    /// Handle a slash command
    pub fn handle_slash_command(&mut self, command: &str) {
        match command.trim() {
            "/help" => {
                let content = include_str!("./content/help.md");

                self.events.push(ConversationEvent::OutOfBandOutput {
                    name: "System".to_string(),
                    command: Some("/help".to_string()),
                    content: content.to_string(),
                });
            }
            _ => self.events.push(ConversationEvent::OutOfBandOutput {
                name: "System".to_string(),
                command: None,
                content: (format!("Unknown command: {command}")),
            }),
        }
    }
}

/// Ephemeral UI/presentation state
#[derive(Debug)]
pub(crate) struct Interaction {
    /// Current application mode
    pub mode: AppMode,
    /// Whether the input is blank
    pub is_input_blank: bool,
    /// True when user has pressed Enter once on a dangerous command
    pub confirmation_pending: bool,
    /// Current streaming status
    pub streaming_status: Option<StreamingStatus>,
    /// Whether current turn was interrupted by user
    pub was_interrupted: bool,
    /// Current error message
    pub error: Option<String>,
}

impl Interaction {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            is_input_blank: false,
            confirmation_pending: false,
            streaming_status: None,
            was_interrupted: false,
            error: None,
        }
    }
}

/// Top-level session state
///
/// Decomposed into `Conversation` (event log + session ID) and
/// `Interaction` (ephemeral UI state). Session methods that cross
/// both sub-structs live here.
#[derive(Debug)]
pub(crate) struct Session {
    pub conversation: Conversation,
    pub interaction: Interaction,
    /// Tracks all tool calls through their full lifecycle.
    pub tool_tracker: ToolTracker,
    /// Whether the session is running inside a git project (for permission UI labels).
    pub in_git_project: bool,
    /// Exit action (set when exiting)
    pub exit_action: Option<ExitAction>,
    /// Abort handle for the active streaming task, if any
    pub stream_abort: Option<AbortHandle>,
}

impl Session {
    pub fn new(in_git_project: bool) -> Self {
        Self {
            conversation: Conversation::new(),
            interaction: Interaction::new(),
            tool_tracker: ToolTracker::new(),
            in_git_project,
            exit_action: None,
            stream_abort: None,
        }
    }

    // ===== Generation lifecycle methods =====

    /// Start generating from submitted input
    pub fn start_generating(&mut self, input: String) {
        self.conversation
            .events
            .push(ConversationEvent::UserMessage { content: input });
        self.interaction.mode = AppMode::Generating;
    }

    /// Generation error occurred
    #[expect(dead_code)]
    pub fn generation_error(&mut self, error: String) {
        self.interaction.error = Some(error);
        self.interaction.mode = AppMode::Error;
    }

    /// Cancel during generation
    pub fn cancel_generation(&mut self) {
        if let Some(abort) = self.stream_abort.take() {
            abort.abort();
        }
        if let Some(ConversationEvent::UserMessage { .. }) = self.conversation.events.last() {
            self.conversation.events.pop();
        }
        self.interaction.mode = AppMode::Input;
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response.
    /// Pushes an empty Text event that will be mutated in-place as chunks arrive.
    pub fn start_streaming(&mut self) {
        self.conversation.events.push(ConversationEvent::Text {
            content: String::new(),
        });
        self.interaction.streaming_status = None;
        self.interaction.was_interrupted = false;
        self.interaction.mode = AppMode::Streaming;
    }

    /// Update streaming status from SSE event
    pub fn update_streaming_status(&mut self, status: &str) {
        self.interaction.streaming_status = Some(StreamingStatus::from_status_str(status));
    }

    /// Cancel streaming with context preservation
    pub fn cancel_streaming(&mut self) {
        if let Some(abort) = self.stream_abort.take() {
            abort.abort();
        }
        self.interaction.was_interrupted = true;

        if let Some(content) = self.conversation.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            if trimmed.is_empty() {
                // Remove the empty text event
                *content = String::new();
            } else {
                *content = format!("{trimmed}\n\n[User cancelled this generation]");
            }
        }
        // Remove trailing empty Text events
        self.conversation.remove_empty_trailing_text();

        self.interaction.streaming_status = None;
        self.interaction.confirmation_pending = false;
        self.interaction.mode = AppMode::Input;
    }

    /// Add a tool call event during streaming.
    /// The current streaming text is already in events, so we just push the tool call.
    pub fn add_tool_call(&mut self, id: String, name: String, input: serde_json::Value) {
        // Trim the streaming text event
        if let Some(content) = self.conversation.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            *content = trimmed;
        }
        self.conversation.remove_empty_trailing_text();

        let is_suggest_command = name == "suggest_command";
        self.conversation
            .events
            .push(ConversationEvent::ToolCall { id, name, input });

        if is_suggest_command {
            self.interaction.streaming_status = None;
            self.interaction.mode = AppMode::Input;
        }
    }

    /// Finalize streaming — trim the accumulated text and change mode
    pub fn finalize_streaming(&mut self) {
        if let Some(content) = self.conversation.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            *content = trimmed;
        }
        self.conversation.remove_empty_trailing_text();
        self.interaction.streaming_status = None;
        self.interaction.mode = AppMode::Input;
    }

    /// Streaming error — remove the partial text event
    pub fn streaming_error(&mut self, error: String) {
        self.conversation.remove_empty_trailing_text();
        self.interaction.error = Some(error);
        self.interaction.mode = AppMode::Error;
    }

    pub(crate) fn handle_client_tool_call(
        &mut self,
        id: String,
        tool: ClientToolCall,
        input: serde_json::Value,
    ) {
        let desc = tool.descriptor();
        let name = desc.canonical_names[0].to_string();

        self.tool_tracker.insert(id.clone(), tool);

        // Add the ToolCall event to the conversation immediately so it appears
        // in the view. Preview data is sourced from tool_tracker.
        self.conversation
            .events
            .push(ConversationEvent::ToolCall { id, name, input });

        // Client tool calls can only happen at the last part of a turn
        self.interaction.streaming_status = None;
        self.interaction.mode = AppMode::Input;
    }

    /// Retry after error
    pub fn retry(&mut self) {
        self.interaction.error = None;
        self.interaction.mode = AppMode::Generating;
    }

    // ===== Tool lifecycle methods =====

    /// Finish a tool call: transition tracker to Completed, push ToolResult to conversation.
    ///
    /// For shell commands, captures the final preview from the ExecutingWithPreview phase
    /// and patches exit_code/interrupted from the authoritative ToolOutcome.
    pub fn finish_tool_call(&mut self, tool_id: &str, outcome: ToolOutcome) {
        let mut preview = self.tool_tracker.get(tool_id).and_then(|t| t.preview());

        // Patch preview with authoritative outcome data (handles race where
        // final VT100 update hasn't been applied yet).
        if let Some(ref mut p) = preview
            && let ToolOutcome::Structured {
                exit_code,
                interrupted,
                ..
            } = &outcome
        {
            p.interrupted = *interrupted;
            if p.exit_code.is_none() {
                p.exit_code = *exit_code;
            }
        }

        // Transition tracker entry to Completed
        if let Some(tracked) = self.tool_tracker.get_mut(tool_id) {
            tracked.complete(preview);
        }

        let content = outcome.format_for_llm();
        let is_error = outcome.is_error();
        self.conversation
            .add_tool_result(tool_id.to_string(), content, is_error, false, None);
    }

    /// Get the footer text for current mode
    pub fn footer_text(&self) -> &'static str {
        match self.interaction.mode {
            AppMode::Input => {
                if self.conversation.has_any_command() && self.interaction.is_input_blank {
                    if self.interaction.confirmation_pending {
                        "[Enter] Confirm dangerous command  [Esc] Cancel"
                    } else {
                        "[Enter] Execute suggested command  [Tab] Insert Command"
                    }
                } else {
                    "[Enter] Send  [Shift+Enter] New line  [Esc] Exit"
                }
            }
            AppMode::Generating | AppMode::Streaming => "[Esc] Cancel",
            AppMode::Error => "[Enter]/[r] Retry  [Esc] Exit",
        }
    }

    /// Check if the application is exiting
    pub fn is_exiting(&self) -> bool {
        self.exit_action.is_some()
    }
}
