use serde::{Deserialize, Serialize};

/// Represents a ssnapshot of some terminal screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    pub screen_dims: (u16, u16),
    pub cursor_pos: (u16, u16),
    pub rows: Vec<String>,
}

impl ScreenSnapshot {
    #[must_use]
    pub fn new(screen_dims: (u16, u16), cursor_pos: (u16, u16), rows: Vec<String>) -> Self {
        Self {
            screen_dims,
            cursor_pos,
            rows,
        }
    }

    #[must_use]
    pub fn row_count(&self) -> u16 {
        self.screen_dims.0
    }

    #[must_use]
    pub fn col_count(&self) -> u16 {
        self.screen_dims.1
    }

    #[must_use]
    pub fn cursor_row(&self) -> u16 {
        self.cursor_pos.0
    }

    #[must_use]
    pub fn cursor_col(&self) -> u16 {
        self.cursor_pos.1
    }
}
