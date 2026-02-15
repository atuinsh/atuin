use super::state::{AppMode, AppState, ExitAction};
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
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
