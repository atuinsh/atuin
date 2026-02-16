//! View model types for the TUI application
//!
//! This module contains the view model types that represent the rendering
//! specification. These types are derived from the domain state (conversation
//! events) via the `Blocks::from_state()` function.

use super::state::{AppMode, AppState, ConversationEvent};

/// Content variants for blocks - each variant is fully self-describing
#[derive(Debug, Clone)]
pub enum Content {
    Input {
        text: String,
        active: bool,
        cursor_pos: usize,
    },
    /// Command suggestion (from suggest_command tool call)
    Command {
        text: String,
        faded: bool, // Phase 5 feature
    },
    Text {
        markdown: String,
    },
    Error {
        message: String,
    },
    Spinner {
        frame: usize,        // 0-3 for animation
        status_text: String, // Status-based text (Processing..., Thinking..., etc.)
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

/// Check if any turn in the conversation has a command
fn has_any_command(events: &[ConversationEvent]) -> bool {
    events.iter().any(|e| {
        if let ConversationEvent::ToolCall { name, input, .. } = e {
            name == "suggest_command" && input.get("command").and_then(|v| v.as_str()).is_some()
        } else {
            false
        }
    })
}

impl Blocks {
    /// Pure function: derive the complete view model from state
    ///
    /// Iterates through conversation events and builds visual blocks.
    /// Also handles streaming text and mode-dependent UI.
    pub fn from_state(state: &AppState) -> Self {
        let mut items = Vec::new();

        // 1. Build blocks from conversation events
        for event in &state.events {
            match event {
                ConversationEvent::UserMessage { content } => {
                    items.push(Block {
                        content: vec![Content::Input {
                            text: content.clone(),
                            active: false,
                            cursor_pos: 0,
                        }],
                        separator_above: false,
                        title: None,
                    });
                }
                ConversationEvent::Text { content } => {
                    items.push(Block {
                        content: vec![Content::Text {
                            markdown: content.clone(),
                        }],
                        separator_above: false,
                        title: None,
                    });
                }
                ConversationEvent::ToolCall { name, input, .. } => {
                    // Only render suggest_command tool calls
                    if name == "suggest_command" {
                        // Extract description/message for text display
                        let description = input.get("description").and_then(|v| v.as_str());

                        // Check if command is present and non-null
                        let command = input.get("command").and_then(|v| v.as_str());

                        // Build block content
                        let mut block_content = Vec::new();

                        // Add text from description if present
                        if let Some(desc) = description {
                            block_content.push(Content::Text {
                                markdown: desc.to_string(),
                            });
                        }

                        // Add command only if non-null
                        if let Some(cmd) = command {
                            block_content.push(Content::Command {
                                text: cmd.to_string(),
                                faded: false,
                            });
                        }

                        // Only add block if there's content
                        if !block_content.is_empty() {
                            items.push(Block {
                                content: block_content,
                                separator_above: false,
                                title: None,
                            });
                        }
                    }
                    // Other tool calls are not rendered (internal protocol)
                }
                ConversationEvent::ToolResult { .. } => {
                    // Tool results are not rendered (internal protocol)
                }
            }
        }

        // 2. Streaming text (if any) - shown during streaming mode
        if state.mode == AppMode::Streaming {
            if state.streaming_text.is_empty() {
                // No content yet - show spinner with status
                let status_text = state
                    .streaming_status
                    .as_ref()
                    .map(|s| s.display_text().to_string())
                    .unwrap_or_else(|| "Generating...".to_string());

                items.push(Block {
                    content: vec![Content::Spinner {
                        frame: state.spinner_frame,
                        status_text,
                    }],
                    separator_above: false,
                    title: None,
                });
            } else {
                // Show streaming text
                items.push(Block {
                    content: vec![Content::Text {
                        markdown: state.streaming_text.clone(),
                    }],
                    separator_above: false,
                    title: None,
                });
            }
        }

        // 3. Mode-dependent UI
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
                let status_text = state
                    .streaming_status
                    .as_ref()
                    .map(|s| s.display_text().to_string())
                    .unwrap_or_else(|| "Generating...".to_string());

                items.push(Block {
                    content: vec![Content::Spinner {
                        frame: state.spinner_frame,
                        status_text,
                    }],
                    separator_above: false,
                    title: None,
                });
            }
            AppMode::Streaming => {
                // Handled above in streaming text section
            }
            AppMode::Review | AppMode::Error => {
                // No additional UI elements
            }
        }

        // 4. Error if present (renders at end)
        if let Some(ref err) = state.error {
            items.push(Block {
                content: vec![Content::Error {
                    message: err.clone(),
                }],
                separator_above: false,
                title: None,
            });
        }

        // 5. Set separator flags (first has no separator)
        for (idx, block) in items.iter_mut().enumerate() {
            block.separator_above = idx > 0;
        }

        // 6. Set title on first block only
        if let Some(first) = items.first_mut() {
            first.title = Some("Describe the command you'd like to generate:".to_string());
        }

        // 7. Derive footer from mode and events
        let footer = Self::footer_for_mode(&state.mode, &state.events);

        Self { items, footer }
    }

    /// Derive footer text from current mode and conversation state
    fn footer_for_mode(mode: &AppMode, events: &[ConversationEvent]) -> &'static str {
        match mode {
            AppMode::Input => "[Enter]: Accept  [Esc]: Cancel",
            AppMode::Generating | AppMode::Streaming => "[Esc]: Cancel",
            AppMode::Review => {
                // Check if any command exists in conversation history
                if has_any_command(events) {
                    "[Enter]: Run  [Tab]: Insert  [e]: Edit  [Esc]: Cancel"
                } else {
                    "[e]: Edit  [Esc]: Cancel"
                }
            }
            AppMode::Error => "[Enter]/[r]: Retry  [Esc]: Cancel",
        }
    }
}
