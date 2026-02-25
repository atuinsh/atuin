use super::state::{AppMode, AppState, ExitAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{Input, Key};

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
        match self.state.mode {
            AppMode::Input => self.handle_input_key(key),
            AppMode::Generating => self.handle_generating_key(key),
            AppMode::Streaming => self.handle_streaming_key(key),
            AppMode::Review => self.handle_review_key(key),
            AppMode::Error => self.handle_error_key(key),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> bool {
        // Handle special keys ourselves
        match key.code {
            KeyCode::Esc => {
                self.state.exit(ExitAction::Cancel);
                return true;
            }
            KeyCode::Enter => {
                if self.state.input_is_empty() {
                    self.state.exit(ExitAction::Cancel);
                } else {
                    self.state.start_generating();
                }
                return true;
            }
            _ => {}
        }

        // Delegate all other keys to textarea
        // Manually convert crossterm KeyEvent to tui-textarea Input
        // (needed due to crossterm version mismatch)
        let tui_key = match key.code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Tab => Key::Tab,
            _ => Key::Null,
        };

        if tui_key != Key::Null {
            let input = Input {
                key: tui_key,
                ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
                alt: key.modifiers.contains(KeyModifiers::ALT),
                shift: key.modifiers.contains(KeyModifiers::SHIFT),
            };
            self.state.textarea.input(input);
        }
        true
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
