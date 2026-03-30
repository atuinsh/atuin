//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. Conversation events match the API protocol format.

use tokio::task::AbortHandle;

/// Streaming status indicators from server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingStatus {
    Processing,
    Searching,
    Thinking,
    WaitingForTools,
}

impl StreamingStatus {
    pub fn from_status_str(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "searching" => Self::Searching,
            "waiting_for_tools" => Self::WaitingForTools,
            _ => Self::Thinking,
        }
    }

    pub fn display_text(&self) -> &'static str {
        match self {
            Self::Processing => "Processing...",
            Self::Searching => "Searching...",
            Self::Thinking => "Thinking...",
            Self::WaitingForTools => "Waiting for tools...",
        }
    }
}

/// Conversation event types matching the API protocol
#[derive(Debug, Clone)]
pub enum ConversationEvent {
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
    /// Tool result (usually from server-side execution)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Out-of-band output from the system - not sent to the server
    OutOfBandOutput {
        name: String,
        command: Option<String>,
        content: String,
    },
}

impl ConversationEvent {
    /// Convert to JSON for API calls
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ConversationEvent::UserMessage { content } => serde_json::json!({
                "type": "user_message",
                "content": content
            }),
            ConversationEvent::Text { content } => serde_json::json!({
                "type": "text",
                "content": content
            }),
            ConversationEvent::ToolCall { id, name, input } => serde_json::json!({
                "type": "tool_call",
                "id": id,
                "name": name,
                "input": input
            }),
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error
            }),
            ConversationEvent::OutOfBandOutput {
                name,
                command,
                content,
            } => serde_json::json!({
                "type": "out_of_band_output",
                "name": name,
                "command": command,
                "content": content
            }),
        }
    }

    /// Extract command from a suggest_command tool call
    pub fn as_command(&self) -> Option<&str> {
        if let ConversationEvent::ToolCall { name, input, .. } = self
            && name == "suggest_command"
        {
            return input.get("command").and_then(|v| v.as_str());
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum AppMode {
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
pub enum ExitAction {
    /// Run the command
    Execute(String),
    /// Insert command without running
    Insert(String),
    /// User canceled
    Cancel,
}

/// Application state — the domain model
///
/// Conversation is stored as a sequence of events matching the API protocol.
/// The view function derives the UI from this state.
#[derive(Debug)]
pub struct AppState {
    /// Current application mode
    pub mode: AppMode,
    /// Conversation events (source of truth, matches API protocol)
    pub events: Vec<ConversationEvent>,
    /// Current error message
    pub error: Option<String>,
    /// Exit action (set when exiting)
    pub exit_action: Option<ExitAction>,
    /// Session ID from server
    pub session_id: Option<String>,
    /// Current streaming status
    pub streaming_status: Option<StreamingStatus>,
    /// Whether the input is blank
    pub is_input_blank: bool,
    /// Whether current turn was interrupted by user
    pub was_interrupted: bool,
    /// True when user has pressed Enter once on a dangerous command
    pub confirmation_pending: bool,
    /// Abort handle for the active streaming task, if any
    pub stream_abort: Option<AbortHandle>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            events: Vec::new(),
            error: None,
            exit_action: None,
            session_id: None,
            streaming_status: None,
            is_input_blank: false,
            was_interrupted: false,
            confirmation_pending: false,
            stream_abort: None,
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
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                    i += 1;
                }
                ConversationEvent::ToolCall { .. } => {
                    let mut tool_uses = Vec::new();
                    while i < events.len() {
                        if let ConversationEvent::ToolCall { id, name, input } = &events[i] {
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
                } => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error
                        }]
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

    // ===== Generation lifecycle methods =====

    /// Start generating from submitted input
    pub fn start_generating(&mut self, input: String) {
        self.events
            .push(ConversationEvent::UserMessage { content: input });
        self.mode = AppMode::Generating;
    }

    /// Generation error occurred
    pub fn generation_error(&mut self, error: String) {
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    /// Cancel during generation
    pub fn cancel_generation(&mut self) {
        if let Some(abort) = self.stream_abort.take() {
            abort.abort();
        }
        if let Some(ConversationEvent::UserMessage { .. }) = self.events.last() {
            self.events.pop();
        }
        self.mode = AppMode::Input;
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response.
    /// Pushes an empty Text event that will be mutated in-place as chunks arrive.
    pub fn start_streaming(&mut self) {
        self.events.push(ConversationEvent::Text {
            content: String::new(),
        });
        self.streaming_status = None;
        self.was_interrupted = false;
        self.mode = AppMode::Streaming;
    }

    /// Store session ID from server response
    pub fn store_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    /// Update streaming status from SSE event
    pub fn update_streaming_status(&mut self, status: &str) {
        self.streaming_status = Some(StreamingStatus::from_status_str(status));
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

    /// Cancel streaming with context preservation
    pub fn cancel_streaming(&mut self) {
        if let Some(abort) = self.stream_abort.take() {
            abort.abort();
        }
        self.was_interrupted = true;

        if let Some(content) = self.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            if trimmed.is_empty() {
                // Remove the empty text event
                *content = String::new();
            } else {
                *content = format!("{trimmed}\n\n[User cancelled this generation]");
            }
        }
        // Remove trailing empty Text events
        self.remove_empty_trailing_text();

        self.streaming_status = None;
        self.confirmation_pending = false;
        self.mode = AppMode::Input;
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

    /// Add a tool call event during streaming.
    /// The current streaming text is already in events, so we just push the tool call.
    pub fn add_tool_call(&mut self, id: String, name: String, input: serde_json::Value) {
        // Trim the streaming text event
        if let Some(content) = self.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            *content = trimmed;
        }
        self.remove_empty_trailing_text();

        let is_suggest_command = name == "suggest_command";
        self.events
            .push(ConversationEvent::ToolCall { id, name, input });

        if is_suggest_command {
            self.streaming_status = None;
            self.mode = AppMode::Input;
        }
    }

    /// Add a tool result event during streaming
    pub fn add_tool_result(&mut self, tool_use_id: String, content: String, is_error: bool) {
        self.events.push(ConversationEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }

    /// Finalize streaming — trim the accumulated text and change mode
    pub fn finalize_streaming(&mut self) {
        if let Some(content) = self.streaming_content_mut() {
            let trimmed = content.trim_start().to_string();
            *content = trimmed;
        }
        self.remove_empty_trailing_text();
        self.streaming_status = None;
        self.mode = AppMode::Input;
    }

    /// Streaming error — remove the partial text event
    pub fn streaming_error(&mut self, error: String) {
        self.remove_empty_trailing_text();
        self.error = Some(error);
        self.mode = AppMode::Error;
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

    // ===== Edit mode and exit methods =====

    /// Start edit mode for refinement
    pub fn start_edit_mode(&mut self) {
        self.confirmation_pending = false;
        self.mode = AppMode::Input;
    }

    /// Retry after error
    pub fn retry(&mut self) {
        self.error = None;
        self.mode = AppMode::Generating;
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

    // ===== Query methods =====

    /// Get the most recent command from events
    pub fn current_command(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| e.as_command())
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

    /// Count non-suggest_command tool calls since the last user message
    pub fn tool_count_since_last_user(&self) -> usize {
        let last_user_idx = self
            .events
            .iter()
            .rposition(|e| matches!(e, ConversationEvent::UserMessage { .. }))
            .unwrap_or(0);

        let mut completed = 0;
        let mut in_flight = false;

        for event in &self.events[last_user_idx..] {
            match event {
                ConversationEvent::ToolCall { name, .. } if name != "suggest_command" => {
                    if in_flight {
                        completed += 1;
                    }
                    in_flight = true;
                }
                ConversationEvent::ToolResult { .. } => {
                    if in_flight {
                        completed += 1;
                        in_flight = false;
                    }
                }
                _ => {}
            }
        }

        completed
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

    /// Get the footer text for current mode
    pub fn footer_text(&self) -> &'static str {
        match self.mode {
            AppMode::Input => {
                if self.has_any_command() && self.is_input_blank {
                    if self.confirmation_pending {
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

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
