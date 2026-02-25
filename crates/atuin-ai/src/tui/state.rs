//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. Conversation events match the API protocol format.

use std::time::Instant;
use tui_textarea::TextArea;

use super::spinner::{ACTIVE_SPINNER, active_tick_interval};

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
    /// Active text input (uses tui-textarea for proper cursor handling)
    pub textarea: TextArea<'static>,
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
    /// Spinner animation state
    pub spinner_frame: usize,
    /// When spinner frame last advanced (for timing control)
    pub last_spinner_tick: Instant,
    /// When streaming started (for spinner delay)
    pub streaming_started: Option<Instant>,
    /// True when user has pressed Enter once on a dangerous command
    pub confirmation_pending: bool,
}

/// Create a TextArea with our preferred configuration
fn create_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    // Disable underline on cursor line - it's distracting
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    // Enable word wrapping
    textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
    textarea
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            events: Vec::new(),
            streaming_text: String::new(),
            textarea: create_textarea(),
            error: None,
            should_exit: false,
            exit_action: None,
            session_id: None,
            streaming_status: None,
            was_interrupted: false,
            spinner_frame: 0,
            last_spinner_tick: Instant::now(),
            streaming_started: None,
            confirmation_pending: false,
        }
    }

    /// Get the current input text
    pub fn input(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Check if input is empty
    pub fn input_is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    /// Clear the input
    pub fn clear_input(&mut self) {
        self.textarea = create_textarea();
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

    // ===== Generation lifecycle methods =====

    /// Start generating from current input
    pub fn start_generating(&mut self) {
        // Add user message event
        self.events.push(ConversationEvent::UserMessage {
            content: self.input(),
        });

        // Clear input, switch mode
        self.clear_input();
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
            tool_input["danger"] = serde_json::json!("high");
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
        self.clear_input();
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response
    pub fn start_streaming(&mut self) {
        self.streaming_text.clear();
        self.streaming_status = None;
        self.was_interrupted = false;
        self.streaming_started = Some(Instant::now());
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
        // Trim leading whitespace since LLM responses often start with \n\n
        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            let interrupted_text = format!("{trimmed}\n\n[User cancelled this generation]");
            self.events.push(ConversationEvent::Text {
                content: interrupted_text,
            });
        }

        // Clear status and return to input
        self.streaming_status = None;
        self.confirmation_pending = false;
        self.mode = AppMode::Input;
    }

    /// Append text chunk during streaming
    /// Trims leading whitespace from the first chunk(s) since LLM responses often start with \n\n
    pub fn append_streaming_text(&mut self, chunk: &str) {
        if self.streaming_text.is_empty() {
            // First chunk(s): trim leading whitespace
            let trimmed = chunk.trim_start();
            if !trimmed.is_empty() {
                self.streaming_text.push_str(trimmed);
            }
        } else {
            // Subsequent chunks: append as-is
            self.streaming_text.push_str(chunk);
        }
    }

    /// Add a tool call event during streaming
    /// Flushes any pending streaming text first to maintain correct event order
    /// For suggest_command, also transitions to Review mode since that ends the LLM turn
    pub fn add_tool_call(&mut self, id: String, name: String, input: serde_json::Value) {
        // Flush streaming text before adding tool call to maintain correct order
        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            self.events.push(ConversationEvent::Text {
                content: trimmed.to_string(),
            });
        }

        // suggest_command marks the end of the LLM turn - transition to Review
        let is_suggest_command = name == "suggest_command";

        self.events
            .push(ConversationEvent::ToolCall { id, name, input });

        if is_suggest_command {
            self.streaming_status = None;
            self.streaming_started = None;
            self.mode = AppMode::Review;
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

    /// Finalize streaming - flush accumulated text to event
    pub fn finalize_streaming(&mut self) {
        // Flush streaming text to a Text event if non-empty
        // Trim leading whitespace since LLM responses often start with \n\n
        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            self.events.push(ConversationEvent::Text {
                content: trimmed.to_string(),
            });
        }
        self.streaming_status = None;
        self.streaming_started = None;
        self.mode = AppMode::Review;
    }

    /// Streaming error
    pub fn streaming_error(&mut self, error: String) {
        // Discard any partial streaming text
        self.streaming_text.clear();
        self.streaming_started = None;
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    // ===== Edit mode and exit methods =====

    /// Start edit mode for refinement
    pub fn start_edit_mode(&mut self) {
        self.confirmation_pending = false;
        self.clear_input();
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

    /// Advance spinner frame if enough time has passed
    /// Called on every event loop tick (50ms), but only advances spinner
    /// when the active spinner's interval has elapsed
    pub fn tick(&mut self) {
        let interval = active_tick_interval();
        if self.last_spinner_tick.elapsed() >= interval {
            self.spinner_frame = (self.spinner_frame + 1) % ACTIVE_SPINNER.frame_count();
            self.last_spinner_tick = Instant::now();
        }
    }

    /// Get the most recent command from events
    pub fn current_command(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| e.as_command())
    }

    /// Check if the most recent command suggestion is marked dangerous
    /// Checks the `danger` field for "high", "medium", or "med" values
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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
