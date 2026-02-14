pub mod state;
pub use state::{BlockKind, BlockState};

#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    pub state: BlockState,
    pub content: String,
}

impl Block {
    pub fn new_input(content: String) -> Self {
        Self {
            kind: BlockKind::Input,
            state: BlockState::Active,
            content,
        }
    }

    pub fn new_building_spinner() -> Self {
        Self {
            kind: BlockKind::Spinner,
            state: BlockState::new_building(),
            content: String::new(),
        }
    }

    pub fn new_building_command() -> Self {
        Self {
            kind: BlockKind::Command,
            state: BlockState::new_building(),
            content: String::new(),
        }
    }

    pub fn new_building_text() -> Self {
        Self {
            kind: BlockKind::Text,
            state: BlockState::new_building(),
            content: String::new(),
        }
    }

    pub fn new_error(message: String) -> Self {
        Self {
            kind: BlockKind::Error,
            state: BlockState::Static,
            content: message,
        }
    }

    pub fn new_streaming_text() -> Self {
        Self {
            kind: BlockKind::Text,
            state: BlockState::Streaming,
            content: String::new(),
        }
    }
}
