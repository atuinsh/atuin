use crate::tui::blocks::{Block, BlockKind, BlockState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// User is typing input
    Input,
    /// Waiting for generation (showing spinner)
    Generating,
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
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
