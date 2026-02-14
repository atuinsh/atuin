#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockState {
    /// Currently being built (streaming content, shows spinner or streaming text)
    Building { spinner_idx: usize },
    /// Finished building, user can interact
    Active,
    /// Previous block, no longer interactive
    Static,
}

impl BlockState {
    pub fn new_building() -> Self {
        BlockState::Building { spinner_idx: 0 }
    }

    pub fn is_building(&self) -> bool {
        matches!(self, BlockState::Building { .. })
    }

    pub fn is_active(&self) -> bool {
        matches!(self, BlockState::Active)
    }

    pub fn is_static(&self) -> bool {
        matches!(self, BlockState::Static)
    }

    /// Transition Building -> Active (called when server sends "done" or stream closes)
    pub fn finish_building(self) -> Self {
        match self {
            BlockState::Building { .. } => BlockState::Active,
            other => other, // Already finished, no-op
        }
    }

    /// Transition Active -> Static (called when new block starts building)
    pub fn make_static(self) -> Self {
        match self {
            BlockState::Building { .. } | BlockState::Active => BlockState::Static,
            BlockState::Static => BlockState::Static,
        }
    }

    /// Advance spinner (only meaningful for Building state)
    pub fn tick(&mut self) {
        if let BlockState::Building { spinner_idx } = self {
            *spinner_idx = (*spinner_idx + 1) % 4; // 4 spinner frames
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// User input with ">" symbol
    Input,
    /// Generated command with "$" symbol
    Command,
    /// Loading indicator
    Spinner,
    /// Explanatory text or error
    Text,
}
