//! View model types for the TUI application
//!
//! This module contains the view model types that represent the rendering
//! specification. These types are the "what the app shows" and are derived
//! from the domain state in state.rs via the `Blocks::from_state()` function.

use super::state::{AppMode, AppState, MessageRole};

/// Content variants for blocks - each variant is fully self-describing
#[derive(Debug, Clone)]
pub enum Content {
    Input {
        text: String,
        active: bool,
        cursor_pos: usize,
    },
    Command {
        text: String,
        faded: bool, // Phase 5 feature, included now for structure
    },
    Text {
        markdown: String,
    },
    Error {
        message: String,
    },
    Spinner {
        frame: usize, // 0-3 for animation
    },
}

impl Content {
    /// Get the prefix symbol for this content type
    pub fn prefix_symbol(&self) -> &'static str {
        match self {
            Content::Input { .. } => ">",
            Content::Command { .. } => "$",
            Content::Text { .. } => " ",
            Content::Error { .. } => "!",
            Content::Spinner { .. } => "/",
        }
    }
}

/// A visual block in the UI
#[derive(Debug, Clone)]
pub struct Block {
    pub content: Content,
    pub separator_above: bool,
    pub title: Option<String>,
}

/// Complete view model - the rendering specification
#[derive(Debug, Clone)]
pub struct Blocks {
    pub items: Vec<Block>,
    pub footer: &'static str,
}

impl Blocks {
    /// Pure function: derive the complete view model from state
    ///
    /// This is the core of the pure state architecture. The same state
    /// always produces the same view model, making rendering predictable
    /// and debuggable.
    pub fn from_state(state: &AppState) -> Self {
        let mut items = Vec::new();

        // 1. Add historical messages
        for msg in &state.messages {
            // User messages -> Input content
            if msg.role == MessageRole::User {
                items.push(Block {
                    content: Content::Input {
                        text: msg.content.clone(),
                        active: false,
                        cursor_pos: 0,
                    },
                    separator_above: false,
                    title: None,
                });
            }

            // Assistant messages -> Command (if present) + Text
            if msg.role == MessageRole::Assistant {
                if let Some(ref cmd) = msg.command {
                    items.push(Block {
                        content: Content::Command {
                            text: cmd.clone(),
                            faded: false,
                        },
                        separator_above: false,
                        title: None,
                    });
                }
                if !msg.content.is_empty() {
                    items.push(Block {
                        content: Content::Text {
                            markdown: msg.content.clone(),
                        },
                        separator_above: false,
                        title: None,
                    });
                }
            }
        }

        // 2. Mode-dependent UI
        match state.mode {
            AppMode::Input => {
                items.push(Block {
                    content: Content::Input {
                        text: state.input.clone(),
                        active: true,
                        cursor_pos: state.cursor_pos,
                    },
                    separator_above: false,
                    title: None,
                });
            }
            AppMode::Generating => {
                items.push(Block {
                    content: Content::Spinner {
                        frame: state.spinner_frame,
                    },
                    separator_above: false,
                    title: None,
                });
            }
            AppMode::Streaming => {
                // During streaming, last message is being built
                // Already handled in messages loop above
            }
            AppMode::Review | AppMode::Error => {
                // No additional UI elements
            }
        }

        // 3. Error if present (renders at end)
        if let Some(ref err) = state.error {
            items.push(Block {
                content: Content::Error {
                    message: err.clone(),
                },
                separator_above: false,
                title: None,
            });
        }

        // 4. Set separator flags (first has no separator)
        for (idx, block) in items.iter_mut().enumerate() {
            block.separator_above = idx > 0;
        }

        // 5. Set title on first block only
        if let Some(first) = items.first_mut() {
            first.title = Some("Describe the command you'd like to generate:".to_string());
        }

        // 6. Derive footer from mode
        let footer = Self::footer_for_mode(&state.mode);

        Self { items, footer }
    }

    /// Derive footer text from current mode
    fn footer_for_mode(mode: &AppMode) -> &'static str {
        match mode {
            AppMode::Input => "[Enter]: Accept  [Esc]: Cancel",
            AppMode::Generating => "[Esc]: Cancel",
            AppMode::Streaming => "[Esc]: Cancel",
            AppMode::Review => "[Enter]: Run  [Tab]: Insert  [e]: Edit  [Esc]: Cancel",
            AppMode::Error => "[Enter]/[r]: Retry  [Esc]: Cancel",
        }
    }
}
