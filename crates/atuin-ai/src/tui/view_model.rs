//! View model types for the TUI application
//!
//! This module contains the view model types that represent the rendering
//! specification. These types are derived from the domain state (conversation
//! events) via the `Blocks::from_state()` function.

use super::state::{AppMode, AppState, ConversationEvent};

/// Warning classification for command suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// Dangerous command (! indicator, AlertError color)
    Danger,
    /// Low confidence answer (? indicator, AlertWarn color)
    LowConfidence,
}

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
    /// Warning for dangerous or low-confidence commands
    Warning {
        kind: WarningKind,
        text: String,
        pending_confirm: bool, // true when awaiting second Enter
    },
    Spinner {
        frame: usize,        // 0-3 for animation
        status_text: String, // Status-based text (Processing..., Thinking..., etc.)
    },
    /// Tool call status display (in-flight or completed summary)
    ToolStatus {
        /// Number of non-suggest_command tools completed
        completed_count: usize,
        /// Current in-flight tool description (None if all done)
        current_label: Option<String>,
        /// Spinner frame for in-flight display
        frame: usize,
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
            Content::Warning { kind, .. } => match kind {
                WarningKind::Danger => "!",
                WarningKind::LowConfidence => "?",
            },
            Content::Spinner { .. } => "/",
            Content::ToolStatus { current_label, .. } => {
                if current_label.is_some() {
                    "/"
                } else {
                    "\u{2713}"
                } // spinner or checkmark
            }
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

/// Count non-suggest_command tool calls since the last user message
fn count_tool_calls_since_last_user(events: &[ConversationEvent]) -> (usize, Option<String>) {
    let last_user_idx = events
        .iter()
        .rposition(|e| matches!(e, ConversationEvent::UserMessage { .. }))
        .unwrap_or(0);

    let mut completed = 0;
    let mut in_flight: Option<String> = None;

    for event in &events[last_user_idx..] {
        match event {
            ConversationEvent::ToolCall { name, .. } if name != "suggest_command" => {
                // New tool call starts as in-flight
                if in_flight.is_some() {
                    // Previous tool is now completed
                    completed += 1;
                }
                in_flight = Some(name.clone());
            }
            ConversationEvent::ToolResult { .. } => {
                // Tool completed
                if in_flight.is_some() {
                    completed += 1;
                    in_flight = None;
                }
            }
            _ => {}
        }
    }

    (completed, in_flight)
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
                    // In Review mode with completed tool calls, prepend ToolStatus to this Text block
                    let (completed, _) = count_tool_calls_since_last_user(&state.events);
                    let mut block_content = Vec::new();

                    if state.mode == AppMode::Review && completed > 0 {
                        block_content.push(Content::ToolStatus {
                            completed_count: completed,
                            current_label: None,
                            frame: 0,
                        });
                    }

                    block_content.push(Content::Text {
                        markdown: content.clone(),
                    });

                    items.push(Block {
                        content: block_content,
                        separator_above: false,
                        title: None,
                    });
                }
                ConversationEvent::ToolCall { name, input, .. } => {
                    // Only render suggest_command tool calls with a command
                    if name == "suggest_command" {
                        let command = input.get("command").and_then(|v| v.as_str());

                        // Build block content - only render if command is present
                        // When command is null, this is a conversation-only turn and the
                        // response text comes via a separate Text event
                        let mut block_content = Vec::new();

                        if let Some(cmd) = command {
                            block_content.push(Content::Command {
                                text: cmd.to_string(),
                                faded: false,
                            });
                        }

                        // Extract warning data from tool call input
                        // danger: "high" | "medium" | "med" | "low" - high/medium/med trigger warning
                        let danger_level = input
                            .get("danger")
                            .and_then(|v| v.as_str())
                            .unwrap_or("low");
                        let is_dangerous = danger_level == "high"
                            || danger_level == "medium"
                            || danger_level == "med";
                        let danger_notes = input.get("danger_notes").and_then(|v| v.as_str());

                        // confidence: "high" | "medium" | "low" - low triggers warning
                        let confidence_level = input
                            .get("confidence")
                            .and_then(|v| v.as_str())
                            .unwrap_or("high");
                        let is_low_confidence = confidence_level == "low";
                        let confidence_notes =
                            input.get("confidence_notes").and_then(|v| v.as_str());

                        // Add warning content if applicable (danger takes precedence)
                        if is_dangerous {
                            if let Some(notes) = danger_notes {
                                block_content.push(Content::Warning {
                                    kind: WarningKind::Danger,
                                    text: notes.to_string(),
                                    pending_confirm: state.confirmation_pending,
                                });
                            }
                        } else if is_low_confidence && let Some(notes) = confidence_notes {
                            block_content.push(Content::Warning {
                                kind: WarningKind::LowConfidence,
                                text: notes.to_string(),
                                pending_confirm: false, // low confidence doesn't require confirm
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

        // 2. AI response block (tool status + streaming text) - shown during Streaming only
        // In Review mode, ToolStatus is handled inline with ConversationEvent::Text above
        if state.mode == AppMode::Streaming {
            let (completed, in_flight) = count_tool_calls_since_last_user(&state.events);
            let mut response_content = Vec::new();

            // Add tool status if there are any non-suggest_command tools
            if completed > 0 || in_flight.is_some() {
                response_content.push(Content::ToolStatus {
                    completed_count: completed,
                    current_label: in_flight.clone(),
                    frame: state.spinner_frame,
                });
            }

            // Add streaming text or spinner
            if state.streaming_text.is_empty() {
                // Check if enough time has passed to show spinner (200ms delay)
                // Show spinner immediately if status event has arrived
                let should_show_spinner = state.streaming_status.is_some()
                    || state
                        .streaming_started
                        .map(|start| start.elapsed() >= std::time::Duration::from_millis(200))
                        .unwrap_or(true);

                if should_show_spinner && in_flight.is_none() {
                    // Only show generating spinner if no tool is in-flight
                    let status_text = state
                        .streaming_status
                        .as_ref()
                        .map(|s| s.display_text().to_string())
                        .unwrap_or_else(|| "Generating...".to_string());

                    response_content.push(Content::Spinner {
                        frame: state.spinner_frame,
                        status_text,
                    });
                }
            } else {
                // Show streaming text
                response_content.push(Content::Text {
                    markdown: state.streaming_text.clone(),
                });
            }

            // Add the response block if there's any content
            if !response_content.is_empty() {
                items.push(Block {
                    content: response_content,
                    separator_above: false,
                    title: None,
                });
            }
        }

        // 3. Mode-dependent UI
        match state.mode {
            AppMode::Input => {
                // Active input uses TextArea widget, rendered directly
                // We add a placeholder block that will be replaced by textarea rendering
                items.push(Block {
                    content: vec![Content::Input {
                        text: state.input(),
                        active: true,
                        cursor_pos: 0, // Not used for active input - textarea handles cursor
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
            first.title = Some("Ask questions or generate a command:".to_string());
        }

        // 7. Derive footer from mode and events
        let footer = Self::footer_for_mode(&state.mode, &state.events, state.confirmation_pending);

        Self { items, footer }
    }

    /// Derive footer text from current mode and conversation state
    fn footer_for_mode(
        mode: &AppMode,
        events: &[ConversationEvent],
        confirmation_pending: bool,
    ) -> &'static str {
        match mode {
            AppMode::Input => "[Enter]: Accept  [Esc]: Cancel",
            AppMode::Generating | AppMode::Streaming => "[Esc]: Cancel",
            AppMode::Review => {
                if confirmation_pending {
                    "[Enter]: Confirm dangerous command  [Esc]: Cancel"
                } else if has_any_command(events) {
                    "[Enter]: Run  [Tab]: Insert  [f]: Follow-up  [Esc]: Cancel"
                } else {
                    "[f]: Follow-up  [Esc]: Cancel"
                }
            }
            AppMode::Error => "[Enter]/[r]: Retry  [Esc]: Cancel",
        }
    }
}
