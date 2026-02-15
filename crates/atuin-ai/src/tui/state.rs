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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
