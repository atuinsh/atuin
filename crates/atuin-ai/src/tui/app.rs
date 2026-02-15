use super::blocks::{Block, BlockState};
use super::state::{AppMode, AppState, ExitAction, MessageRole};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Thin wrapper around AppState for compatibility
/// All state lives in AppState, this just provides the handle_key interface
pub struct App {
    pub state: AppState,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::new(),
        }
    }

    /// Handle a key event. Returns true if render is needed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Ctrl combinations pass through (Ctrl+C handled by SIGINT)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        match self.state.mode {
            AppMode::Input => self.handle_input_key(key),
            AppMode::Generating => self.handle_generating_key(key),
            AppMode::Streaming => self.handle_streaming_key(key),
            AppMode::Review => self.handle_review_key(key),
            AppMode::Error => self.handle_error_key(key),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.state.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter => {
                if self.state.input.trim().is_empty() {
                    self.state.exit(ExitAction::Cancel);
                } else {
                    self.state.start_generating();
                }
                true
            }
            KeyCode::Backspace => {
                self.state.delete_char();
                true
            }
            KeyCode::Left => {
                self.state.move_cursor_left();
                true
            }
            KeyCode::Right => {
                self.state.move_cursor_right();
                true
            }
            KeyCode::Char(c) => {
                self.state.insert_char(c);
                true
            }
            _ => false,
        }
    }

    fn handle_generating_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.state.cancel_generation();
                true
            }
            _ => false, // Discard other keys during generation
        }
    }

    fn handle_streaming_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                // Cancel streaming, revert to Review
                self.state.mode = AppMode::Review;
                true
            }
            _ => false, // Ignore other keys during streaming
        }
    }

    fn handle_review_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.state.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter => {
                if let Some(cmd) = self.state.current_command() {
                    self.state.exit(ExitAction::Execute(cmd.to_string()));
                }
                true
            }
            KeyCode::Tab => {
                if let Some(cmd) = self.state.current_command() {
                    self.state.exit(ExitAction::Insert(cmd.to_string()));
                }
                true
            }
            KeyCode::Char('e') => {
                self.state.start_edit_mode();
                true
            }
            _ => false,
        }
    }

    fn handle_error_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.state.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter | KeyCode::Char('r') => {
                self.state.retry();
                true
            }
            _ => false,
        }
    }

    // ===== Delegation methods for API compatibility =====
    // These forward to AppState for backward compatibility with inline.rs
    // Will be removed in Plan 03 when inline.rs is updated

    pub fn tick(&mut self) {
        self.state.tick();
    }

    pub fn start_streaming_response(&mut self) {
        self.state.start_streaming_response();
    }

    pub fn append_to_streaming_block(&mut self, chunk: &str) {
        self.state.append_streaming_text(chunk);
    }

    pub fn add_tool_call_event(&mut self, id: String, name: String, input: serde_json::Value) {
        self.state.add_tool_call_event(id, name, input);
    }

    pub fn add_tool_result_event(&mut self, tool_use_id: String, content: String, is_error: bool) {
        self.state
            .add_tool_result_event(tool_use_id, content, is_error);
    }

    pub fn finalize_streaming_with_command(&mut self, command: String) {
        self.state.finalize_streaming_with_command(command);
    }

    pub fn finalize_streaming(&mut self) {
        self.state.finalize_streaming();
    }

    pub fn streaming_error(&mut self, error: String) {
        self.state.streaming_error(error);
    }

    pub fn generation_complete(
        &mut self,
        command: String,
        explanation: Option<String>,
        dangerous: bool,
        warnings: Vec<String>,
    ) {
        self.state
            .generation_complete(command, explanation, dangerous, warnings);
    }

    pub fn generation_error(&mut self, error: String) {
        self.state.generation_error(error);
    }

    // ===== Legacy compatibility properties =====
    // These convert new state to old block format for render.rs
    // Will be removed in Plan 03 when render.rs is updated

    /// Convert messages to legacy blocks for rendering
    pub fn blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();

        // Convert each message to appropriate blocks
        for msg in &self.state.messages {
            match msg.role {
                MessageRole::User => {
                    let mut block = Block::new_input(msg.content.clone());
                    block.state = BlockState::Static;
                    blocks.push(block);
                }
                MessageRole::Assistant => {
                    // Add explanation text if present
                    if !msg.content.is_empty() {
                        let mut text_block = Block::new_building_text();
                        text_block.content = msg.content.clone();
                        text_block.state = BlockState::Static;
                        blocks.push(text_block);
                    }
                    // Add command if present
                    if let Some(ref cmd) = msg.command {
                        let mut cmd_block = Block::new_building_command();
                        cmd_block.content = cmd.clone();
                        cmd_block.state = if self.state.mode == AppMode::Review {
                            BlockState::Active
                        } else {
                            BlockState::Static
                        };
                        blocks.push(cmd_block);
                    }
                }
            }
        }

        // Add mode-specific blocks
        match self.state.mode {
            AppMode::Generating => {
                blocks.push(Block::new_building_spinner());
            }
            AppMode::Streaming => {
                // Streaming message is already in messages as the last assistant message
                if let Some(last) = blocks.last_mut() {
                    last.state = BlockState::Streaming;
                }
            }
            AppMode::Error => {
                if let Some(ref error) = self.state.error {
                    blocks.push(Block::new_error(error.clone()));
                }
            }
            _ => {}
        }

        blocks
    }

    /// Legacy field access properties
    pub fn mode(&self) -> &AppMode {
        &self.state.mode
    }

    pub fn input(&self) -> &str {
        &self.state.input
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
