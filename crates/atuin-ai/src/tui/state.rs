//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. Conversation events match the API protocol format.

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
            _ => Self::Thinking, // Default to thinking for "thinking" and unknown
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
        }
    }

    /// Extract command from a suggest_command tool call
    pub fn as_command(&self) -> Option<&str> {
        if let ConversationEvent::ToolCall { name, input, .. } = self
            && name == "suggest_command"
        {
            // command can be null for pure conversational turns
            return input.get("command").and_then(|v| v.as_str());
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// User is typing input
    Input,
    /// Waiting for generation (showing spinner)
    Generating,
    /// Streaming SSE response
    Streaming,
    /// Reviewing generated command
    Review,
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

/// Application state - the domain model
///
/// Conversation is stored as a sequence of events matching the API protocol.
/// The view model is derived from this state via `Blocks::from_state()`.
pub struct AppState {
    /// Current application mode
    pub mode: AppMode,
    /// Conversation events (source of truth, matches API protocol)
    pub events: Vec<ConversationEvent>,
    /// Text being streamed (accumulated, flushed to Text event on completion)
    pub streaming_text: String,
    /// Current typing buffer
    pub input: String,
    /// Cursor position (character index, not byte index)
    pub cursor_pos: usize,
    /// Current error message (renders at end of blocks)
    pub error: Option<String>,
    /// Whether app should exit
    pub should_exit: bool,
    /// Exit action (set when exiting)
    pub exit_action: Option<ExitAction>,
    /// Session ID from server (store after first response, send on subsequent)
    pub session_id: Option<String>,
    /// Current streaming status (for spinner text)
    pub streaming_status: Option<StreamingStatus>,
    /// Whether current turn was interrupted by user
    pub was_interrupted: bool,
    /// Spinner animation state (0-3)
    pub spinner_frame: usize,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            events: Vec::new(),
            streaming_text: String::new(),
            input: String::new(),
            cursor_pos: 0,
            error: None,
            should_exit: false,
            exit_action: None,
            session_id: None,
            streaming_status: None,
            was_interrupted: false,
            spinner_frame: 0,
        }
    }

    /// Convert conversation events to Claude API message format
    /// Groups consecutive tool calls, handles role alternation
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
                    // Group consecutive tool calls into single assistant message
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
            }
        }

        messages
    }

    /// Convert character position to byte index for string slicing
    pub fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.cursor_pos)
            .unwrap_or(self.input.len())
    }

    /// Insert character at cursor position
    pub fn insert_char(&mut self, c: char) {
        let byte_idx = self.byte_index();
        self.input.insert(byte_idx, c);
        self.cursor_pos += 1;
    }

    /// Delete character before cursor
    pub fn delete_char(&mut self) {
        if self.cursor_pos > 0 {
            let byte_idx = self.byte_index();
            let prev_char_idx = self.input[..byte_idx]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev_char_idx);
            self.cursor_pos -= 1;
        }
    }

    /// Move cursor left (saturating at 0)
    pub fn move_cursor_left(&mut self) {
        self.cursor_pos = self.cursor_pos.saturating_sub(1);
    }

    /// Move cursor right (clamped to string length)
    pub fn move_cursor_right(&mut self) {
        let max = self.input.chars().count();
        self.cursor_pos = (self.cursor_pos + 1).min(max);
    }

    // ===== Generation lifecycle methods =====

    /// Start generating from current input
    pub fn start_generating(&mut self) {
        // Add user message event
        self.events.push(ConversationEvent::UserMessage {
            content: self.input.clone(),
        });

        // Clear input, switch mode
        self.input.clear();
        self.cursor_pos = 0;
        self.mode = AppMode::Generating;
    }

    /// Generation complete with command (legacy method, kept for compatibility)
    pub fn generation_complete(
        &mut self,
        command: String,
        explanation: Option<String>,
        dangerous: bool,
        warnings: Vec<String>,
    ) {
        // Add explanation as text event if present
        if let Some(ref exp) = explanation {
            self.events.push(ConversationEvent::Text {
                content: exp.clone(),
            });
        }

        // Add tool_call event for suggest_command
        let tool_id = format!("gen_{}", uuid::Uuid::new_v4().simple());
        let mut tool_input = serde_json::json!({
            "command": command,
            "conversation_only": false,
            "confidence": "high"
        });
        if let Some(ref exp) = explanation {
            tool_input["message"] = serde_json::json!(exp);
        }
        if dangerous {
            tool_input["dangerous"] = serde_json::json!(true);
        }
        if !warnings.is_empty() {
            tool_input["warning"] = serde_json::json!(warnings.join("; "));
        }

        self.events.push(ConversationEvent::ToolCall {
            id: tool_id,
            name: "suggest_command".to_string(),
            input: tool_input,
        });

        self.mode = AppMode::Review;
    }

    /// Generation error occurred
    pub fn generation_error(&mut self, error: String) {
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    /// Cancel during generation
    pub fn cancel_generation(&mut self) {
        // Remove the last user message since generation was cancelled
        if let Some(ConversationEvent::UserMessage { .. }) = self.events.last() {
            self.events.pop();
        }
        self.mode = AppMode::Input;
        self.input.clear();
        self.cursor_pos = 0;
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response
    pub fn start_streaming(&mut self) {
        self.streaming_text.clear();
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

    /// Cancel streaming with context preservation
    pub fn cancel_streaming(&mut self) {
        // Mark as interrupted
        self.was_interrupted = true;

        // Flush partial text with interruption marker if any
        if !self.streaming_text.is_empty() {
            let interrupted_text = format!(
                "{}\n\n[User cancelled this generation]",
                std::mem::take(&mut self.streaming_text)
            );
            self.events.push(ConversationEvent::Text {
                content: interrupted_text,
            });
        }

        // Clear status and return to input
        self.streaming_status = None;
        self.mode = AppMode::Input;
    }

    /// Append text chunk during streaming
    pub fn append_streaming_text(&mut self, chunk: &str) {
        self.streaming_text.push_str(chunk);
    }

    /// Add a tool call event during streaming
    pub fn add_tool_call(&mut self, id: String, name: String, input: serde_json::Value) {
        self.events
            .push(ConversationEvent::ToolCall { id, name, input });
    }

    /// Add a tool result event during streaming
    pub fn add_tool_result(&mut self, tool_use_id: String, content: String, is_error: bool) {
        self.events.push(ConversationEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }

    /// Finalize streaming - flush accumulated text to event
    pub fn finalize_streaming(&mut self) {
        // Flush streaming text to a Text event if non-empty
        if !self.streaming_text.is_empty() {
            self.events.push(ConversationEvent::Text {
                content: std::mem::take(&mut self.streaming_text),
            });
        }
        self.streaming_status = None;
        self.mode = AppMode::Review;
    }

    /// Streaming error
    pub fn streaming_error(&mut self, error: String) {
        // Discard any partial streaming text
        self.streaming_text.clear();
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    // ===== Edit mode and exit methods =====

    /// Start edit mode for refinement
    pub fn start_edit_mode(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
        self.mode = AppMode::Input;
    }

    /// Exit with action
    pub fn exit(&mut self, action: ExitAction) {
        self.exit_action = Some(action);
        self.should_exit = true;
    }

    /// Retry after error
    pub fn retry(&mut self) {
        self.error = None;
        self.mode = AppMode::Generating;
    }

    // ===== Utility methods =====

    /// Advance spinner frame
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 4;
    }

    /// Get the most recent command from events
    pub fn current_command(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| e.as_command())
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
