//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. These types are the "what the app knows" and are separate from
//! the view model types in view_model.rs which represent "what the app shows".

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    /// User input message
    User,
    /// AI response (may have command)
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    /// If this message includes a suggested command
    pub command: Option<String>,
}

impl Message {
    pub fn user(content: String) -> Self {
        Self {
            role: MessageRole::User,
            content,
            command: None,
        }
    }

    pub fn assistant(content: String, command: Option<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
            command,
        }
    }
}

/// Application state - the domain model
///
/// This struct holds all the data the application knows about.
/// The view model is derived from this state via `Blocks::from_state()`.
pub struct AppState {
    /// Current application mode
    pub mode: AppMode,
    /// Conversation history (primary data)
    pub messages: Vec<Message>,
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
    /// Whether in refine mode vs initial generate
    pub is_refine_mode: bool,
    /// Conversation events for API protocol
    pub conversation_events: Vec<serde_json::Value>,
    /// Spinner animation state (0-3)
    pub spinner_frame: usize,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            error: None,
            should_exit: false,
            exit_action: None,
            is_refine_mode: false,
            conversation_events: Vec::new(),
            spinner_frame: 0,
        }
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
        // Add user message to conversation
        self.conversation_events.push(serde_json::json!({
            "type": "user_message",
            "content": self.input.clone()
        }));

        // Add message to display history
        self.messages.push(Message {
            role: MessageRole::User,
            content: self.input.clone(),
            command: None,
        });

        // Clear input, switch mode
        self.input.clear();
        self.cursor_pos = 0;
        self.mode = AppMode::Generating;
    }

    /// Generation complete with command
    pub fn generation_complete(
        &mut self,
        command: String,
        explanation: Option<String>,
        dangerous: bool,
        warnings: Vec<String>,
    ) {
        // Create assistant message with command
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content: explanation.clone().unwrap_or_default(),
            command: Some(command.clone()),
        });

        // Add events for conversation history (for refine API)
        if let Some(ref exp) = explanation {
            self.conversation_events.push(serde_json::json!({
                "type": "text",
                "content": exp
            }));
        }

        // Add tool_call event
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
        self.conversation_events.push(serde_json::json!({
            "type": "tool_call",
            "id": tool_id,
            "name": "suggest_command",
            "input": tool_input
        }));

        self.mode = AppMode::Review;
    }

    /// Generation error occurred
    pub fn generation_error(&mut self, error: String) {
        // Remove incomplete message if any was being built
        // (Per user decision: remove incomplete message before displaying error)
        if let Some(last) = self.messages.last() {
            if last.role == MessageRole::Assistant
                && last.content.is_empty()
                && last.command.is_none()
            {
                self.messages.pop();
            }
        }

        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    /// Cancel during generation
    pub fn cancel_generation(&mut self) {
        // Just revert mode, keep conversation history intact
        self.mode = AppMode::Input;
        self.input.clear();
        self.cursor_pos = 0;
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response (creates empty assistant message)
    pub fn start_streaming_response(&mut self) {
        // Create empty assistant message that will be populated
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            command: None,
        });
        self.mode = AppMode::Streaming;
    }

    /// Append text to streaming message (mutate in place per user decision)
    pub fn append_streaming_text(&mut self, chunk: &str) {
        if let Some(last) = self.messages.last_mut() {
            if last.role == MessageRole::Assistant {
                last.content.push_str(chunk);
            }
        }
    }

    /// Finalize streaming with command from suggest_command tool
    pub fn finalize_streaming_with_command(&mut self, command: String) {
        // Set command on last assistant message
        if let Some(last) = self.messages.last_mut() {
            if last.role == MessageRole::Assistant {
                last.command = Some(command);
            }
        }

        // Add text event to conversation (for refine)
        if let Some(last) = self.messages.last() {
            if !last.content.is_empty() {
                self.conversation_events.push(serde_json::json!({
                    "type": "text",
                    "content": last.content
                }));
            }
        }

        self.mode = AppMode::Review;
    }

    /// Finalize streaming without command
    pub fn finalize_streaming(&mut self) {
        self.mode = AppMode::Review;
    }

    /// Streaming error
    pub fn streaming_error(&mut self, error: String) {
        // Remove incomplete assistant message per user decision
        if let Some(last) = self.messages.last() {
            if last.role == MessageRole::Assistant {
                self.messages.pop();
            }
        }

        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    /// Add tool_call event
    pub fn add_tool_call_event(&mut self, id: String, name: String, input: serde_json::Value) {
        self.conversation_events.push(serde_json::json!({
            "type": "tool_call",
            "id": id,
            "name": name,
            "input": input
        }));
    }

    /// Add tool_result event
    pub fn add_tool_result_event(&mut self, tool_use_id: String, content: String, is_error: bool) {
        self.conversation_events.push(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error
        }));
    }

    // ===== Edit mode and exit methods =====

    /// Start edit mode for refinement
    pub fn start_edit_mode(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
        self.is_refine_mode = true;
        self.mode = AppMode::Input;
    }

    /// Exit with action
    pub fn exit(&mut self, action: ExitAction) {
        self.exit_action = Some(action);
        self.should_exit = true;
    }

    /// Retry after error
    pub fn retry(&mut self) {
        self.error = None; // Clear error per user decision
        self.mode = AppMode::Generating;
    }

    // ===== Utility methods =====

    /// Advance spinner frame
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 4;
    }

    /// Get the most recent command (for execute/insert)
    pub fn current_command(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find_map(|m| m.command.as_deref())
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
