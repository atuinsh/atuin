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
    /// Assistant response with command and optional explanation in same block
    AssistantResponse {
        command: String,
        explanation: Option<String>,
        faded: bool, // Phase 5 feature
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
            Content::AssistantResponse { .. } => "$",
            Content::Text { .. } => " ",
            Content::Error { .. } => "!",
            Content::Spinner { .. } => "/",
        }
    }
}

/// A visual block in the UI
#[derive(Debug, Clone)]
pub struct Block {
    pub content: Vec<Content>,
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

        // Track if we're streaming the last assistant message
        let is_streaming = state.mode == AppMode::Streaming;
        let msg_count = state.messages.len();

        // 1. Add historical messages
        for (idx, msg) in state.messages.iter().enumerate() {
            let is_last = idx == msg_count.saturating_sub(1);
            let is_streaming_this = is_streaming && is_last && msg.role == MessageRole::Assistant;

            // User messages -> Input content
            if msg.role == MessageRole::User {
                items.push(Block {
                    content: vec![Content::Input {
                        text: msg.content.clone(),
                        active: false,
                        cursor_pos: 0,
                    }],
                    separator_above: false,
                    title: None,
                });
            }

            // Assistant messages -> may have command, text, or both
            if msg.role == MessageRole::Assistant {
                let mut content_items = Vec::new();

                // Add command if present
                if let Some(ref cmd) = msg.command {
                    content_items.push(Content::AssistantResponse {
                        command: cmd.clone(),
                        explanation: None, // Text shown separately below
                        faded: false,
                    });
                }

                // Add text content if present
                if !msg.content.is_empty() {
                    content_items.push(Content::Text {
                        markdown: msg.content.clone(),
                    });
                }

                // If streaming with no content yet, show spinner
                if content_items.is_empty() && is_streaming_this {
                    content_items.push(Content::Spinner {
                        frame: state.spinner_frame,
                    });
                }

                // Only create block if we have content
                if !content_items.is_empty() {
                    items.push(Block {
                        content: content_items,
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
                    content: vec![Content::Input {
                        text: state.input.clone(),
                        active: true,
                        cursor_pos: state.cursor_pos,
                    }],
                    separator_above: false,
                    title: None,
                });
            }
            AppMode::Generating => {
                items.push(Block {
                    content: vec![Content::Spinner {
                        frame: state.spinner_frame,
                    }],
                    separator_above: false,
                    title: None,
                });
            }
            AppMode::Streaming => {
                // Streaming indicator is handled in messages loop above
                // when the assistant message has no content yet
            }
            AppMode::Review | AppMode::Error => {
                // No additional UI elements
            }
        }

        // 3. Error if present (renders at end)
        if let Some(ref err) = state.error {
            items.push(Block {
                content: vec![Content::Error {
                    message: err.clone(),
                }],
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
