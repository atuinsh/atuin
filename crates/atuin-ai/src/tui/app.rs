use crate::tui::blocks::{Block, BlockKind, BlockState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    Execute(String), // Run the command
    Insert(String),  // Insert command without running
    Cancel,          // User canceled
}

pub struct App {
    /// Current application mode
    pub mode: AppMode,
    /// All blocks in conversation (newest last)
    pub blocks: Vec<Block>,
    /// Current input text (when in Input mode)
    pub input: String,
    /// Whether app should exit
    pub should_exit: bool,
    /// Exit action (set when exiting)
    pub exit_action: Option<ExitAction>,
    /// Last error message (when in Error mode)
    pub error_message: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Input,
            blocks: Vec::new(),
            input: String::new(),
            should_exit: false,
            exit_action: None,
            error_message: None,
        }
    }

    /// Get the currently active (non-static) block, if any
    pub fn active_block(&self) -> Option<&Block> {
        self.blocks.iter().rev().find(|b| !b.state.is_static())
    }

    /// Get mutable reference to currently active block
    pub fn active_block_mut(&mut self) -> Option<&mut Block> {
        self.blocks.iter_mut().rev().find(|b| !b.state.is_static())
    }

    /// Make all blocks static (called before adding new block)
    fn make_all_static(&mut self) {
        for block in &mut self.blocks {
            block.state = block.state.clone().make_static();
        }
    }

    /// Start generating (Building state)
    pub fn start_generating(&mut self) {
        // Finalize input block as static
        self.make_all_static();
        // Add input block with current text
        self.blocks.push(Block::new_input(self.input.clone()));
        self.make_all_static();
        // Add spinner block
        self.blocks.push(Block::new_building_spinner());
        self.mode = AppMode::Generating;
    }

    /// Handle generation complete (transition to Review)
    pub fn generation_complete(&mut self, command: String, explanation: Option<String>) {
        // Remove spinner, add command block
        if let Some(pos) = self
            .blocks
            .iter()
            .rposition(|b| b.kind == BlockKind::Spinner)
        {
            self.blocks.remove(pos);
        }
        self.make_all_static();
        let mut cmd_block = Block::new_building_command();
        cmd_block.content = command;
        cmd_block.state = BlockState::Active;
        self.blocks.push(cmd_block);

        // Add explanation as text block if present
        if let Some(exp) = explanation {
            let mut text_block = Block::new_building_text();
            text_block.content = exp;
            text_block.state = BlockState::Active;
            self.blocks.push(text_block);
        }

        self.mode = AppMode::Review;
    }

    /// Handle cancel during generation per user decision:
    /// Remove partial block, revert previous block to Active
    pub fn cancel_generation(&mut self) {
        // Remove any Building blocks
        self.blocks.retain(|b| !b.state.is_building());

        // Revert the last block to Active (if any)
        if let Some(last) = self.blocks.last_mut() {
            last.state = BlockState::Active;
        }

        self.mode = AppMode::Input;
        self.input.clear(); // Or keep previous input? Per discretion, clear.
    }

    /// Handle generation error
    pub fn generation_error(&mut self, error: String) {
        // Remove spinner
        if let Some(pos) = self
            .blocks
            .iter()
            .rposition(|b| b.kind == BlockKind::Spinner)
        {
            self.blocks.remove(pos);
        }
        // Add error block
        self.blocks.push(Block::new_error(error.clone()));
        self.error_message = Some(error);
        self.mode = AppMode::Error;
    }

    /// Tick all building blocks (advance spinners)
    pub fn tick(&mut self) {
        for block in &mut self.blocks {
            block.state.tick();
        }
    }

    /// Exit with action
    pub fn exit(&mut self, action: ExitAction) {
        self.exit_action = Some(action);
        self.should_exit = true;
    }

    /// Start streaming response (creates empty streaming text block)
    pub fn start_streaming_response(&mut self) {
        self.make_all_static();
        self.blocks.push(Block::new_streaming_text());
        self.mode = AppMode::Streaming;
    }

    /// Append text chunk to streaming block
    pub fn append_to_streaming_block(&mut self, chunk: &str) {
        if let Some(block) = self
            .blocks
            .iter_mut()
            .rev()
            .find(|b| b.state.is_streaming())
        {
            block.content.push_str(chunk);
        }
    }

    /// Finalize streaming block (transition to Review mode)
    pub fn finalize_streaming(&mut self) {
        if let Some(block) = self
            .blocks
            .iter_mut()
            .rev()
            .find(|b| b.state.is_streaming())
        {
            block.state = BlockState::Static;
        }
        self.mode = AppMode::Review;
    }

    /// Handle streaming error
    pub fn streaming_error(&mut self, error: String) {
        // Remove streaming block if present
        self.blocks.retain(|b| !b.state.is_streaming());
        self.blocks.push(Block::new_error(error.clone()));
        self.error_message = Some(error);
        self.mode = AppMode::Error;
    }

    /// Handle a key event. Returns true if render is needed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Pass through Ctrl combinations per user decision
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Ctrl+C is handled by SIGINT, other Ctrl combos eaten
            return true;
        }

        match self.mode {
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
                // Esc when idle exits TUI per user decision
                self.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter => {
                if self.input.trim().is_empty() {
                    // Empty input = cancel
                    self.exit(ExitAction::Cancel);
                } else {
                    // Start generation
                    self.start_generating();
                }
                true
            }
            KeyCode::Backspace => {
                self.input.pop();
                true
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                true
            }
            _ => false,
        }
    }

    fn handle_generating_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                // Esc during generation cancels and ignores response
                self.cancel_generation();
                true
            }
            _ => {
                // Keystrokes during streaming are discarded per user decision
                false
            }
        }
    }

    fn handle_review_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter => {
                // Run command
                if let Some(block) = self.blocks.iter().find(|b| b.kind == BlockKind::Command) {
                    self.exit(ExitAction::Execute(block.content.clone()));
                }
                true
            }
            KeyCode::Tab => {
                // Insert command without running
                if let Some(block) = self.blocks.iter().find(|b| b.kind == BlockKind::Command) {
                    self.exit(ExitAction::Insert(block.content.clone()));
                }
                true
            }
            KeyCode::Char('e') => {
                // Edit/refine - reset to input mode (full implementation in Phase 4)
                self.mode = AppMode::Input;
                self.input.clear();
                true
            }
            _ => false,
        }
    }

    fn handle_streaming_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                // Cancel streaming
                self.blocks.retain(|b| !b.state.is_streaming());
                self.mode = AppMode::Review;
                true
            }
            _ => false, // Ignore other keys during streaming
        }
    }

    fn handle_error_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.exit(ExitAction::Cancel);
                true
            }
            KeyCode::Enter | KeyCode::Char('r') => {
                // Retry - re-enter generating mode
                self.error_message = None;
                self.mode = AppMode::Generating;
                self.blocks.push(Block::new_building_spinner());
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
