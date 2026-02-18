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
                self.state.cancel_streaming();
                true
            }
            _ => false, // Ignore other keys during streaming
        }
    }

    fn handle_review_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.state.confirmation_pending = false; // Clear confirmation state
                self.state.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter => {
                let cmd = self.state.current_command().map(|c| c.to_string());
                if let Some(cmd) = cmd {
                    if self.state.is_current_command_dangerous() && !self.state.confirmation_pending
                    {
                        // First Enter on dangerous command: enter confirmation mode
                        self.state.confirmation_pending = true;
                    } else {
                        // Second Enter (confirmation), or non-dangerous command: execute
                        self.state.confirmation_pending = false;
                        self.state.exit(ExitAction::Execute(cmd));
                    }
                }
                true
            }
            KeyCode::Tab => {
                let cmd = self.state.current_command().map(|c| c.to_string());
                if let Some(cmd) = cmd {
                    self.state.confirmation_pending = false; // Clear on Tab too
                    self.state.exit(ExitAction::Insert(cmd));
                }
                true
            }
            KeyCode::Char('f') => {
                // Changed from 'e' to 'f' for follow-up mode
                self.state.confirmation_pending = false; // Clear on follow-up
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
