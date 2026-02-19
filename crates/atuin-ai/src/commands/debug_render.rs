//! Debug render command for TUI development
//!
//! Takes JSON state as input and outputs a single rendered frame as text.
//! Useful for debugging view model derivation and rendering without running the full TUI.

use eyre::{Context, Result};
use ratatui::{Terminal, backend::TestBackend};
use serde::Deserialize;
use std::io::{self, Read};
use std::time::Instant;

use crate::tui::{
    render::{RenderContext, render},
    state::{AppMode, AppState, ConversationEvent, StreamingStatus},
    view_model::Blocks,
};

/// JSON input format for debug rendering
#[derive(Debug, Deserialize)]
pub struct DebugInput {
    /// Conversation events in API format
    pub events: Vec<EventInput>,
    /// Current mode: "Input", "Generating", "Streaming", "Review", "Error"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Text being streamed (for Streaming mode)
    #[serde(default)]
    pub streaming_text: String,
    /// Current input buffer
    #[serde(default)]
    pub input: String,
    /// Cursor position
    #[serde(default)]
    pub cursor_pos: usize,
    /// Spinner frame (0-3)
    #[serde(default)]
    pub spinner_frame: usize,
    /// Error message
    #[serde(default)]
    pub error: Option<String>,
    /// Session ID from server
    #[serde(default)]
    pub session_id: Option<String>,
    /// Streaming status
    #[serde(default)]
    pub streaming_status: Option<String>,
    /// Whether current turn was interrupted
    #[serde(default)]
    pub was_interrupted: bool,
    /// Terminal width for rendering
    #[serde(default = "default_width")]
    pub width: u16,
    /// Terminal height for rendering
    #[serde(default = "default_height")]
    pub height: u16,
}

fn default_mode() -> String {
    "Review".to_string()
}

fn default_width() -> u16 {
    80
}

fn default_height() -> u16 {
    // Default to a reasonable height; state files include calculated height
    50
}

/// Event input matching the API protocol format
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventInput {
    UserMessage {
        content: String,
    },
    Text {
        content: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl From<EventInput> for ConversationEvent {
    fn from(input: EventInput) -> Self {
        match input {
            EventInput::UserMessage { content } => ConversationEvent::UserMessage { content },
            EventInput::Text { content } => ConversationEvent::Text { content },
            EventInput::ToolCall { id, name, input } => {
                ConversationEvent::ToolCall { id, name, input }
            }
            EventInput::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            },
        }
    }
}

impl DebugInput {
    /// Parse JSON from string
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("Failed to parse debug input JSON")
    }

    /// Convert to AppState
    pub fn to_state(&self) -> AppState {
        let mode = match self.mode.as_str() {
            "Input" => AppMode::Input,
            "Generating" => AppMode::Generating,
            "Streaming" => AppMode::Streaming,
            "Review" => AppMode::Review,
            "Error" => AppMode::Error,
            _ => AppMode::Review,
        };

        let events: Vec<ConversationEvent> = self.events.iter().cloned().map(Into::into).collect();

        let streaming_status = self
            .streaming_status
            .as_ref()
            .map(|s| StreamingStatus::from_status_str(s));

        // Create textarea from input and set cursor position
        let mut textarea = tui_textarea::TextArea::from(self.input.lines());
        // Disable underline on cursor line
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        // Enable word wrapping
        textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
        // Note: cursor_pos from old format is character-based; new format has row/col
        // For compatibility, just move to end if we have text
        if !self.input.is_empty() {
            textarea.move_cursor(tui_textarea::CursorMove::End);
        }

        AppState {
            mode,
            events,
            streaming_text: self.streaming_text.clone(),
            textarea,
            error: self.error.clone(),
            should_exit: false,
            exit_action: None,
            session_id: self.session_id.clone(),
            streaming_status,
            was_interrupted: self.was_interrupted,
            spinner_frame: self.spinner_frame,
            last_spinner_tick: Instant::now(),
            streaming_started: None,
            confirmation_pending: false,
        }
    }
}

/// Output format options
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    /// Raw terminal output (ANSI)
    #[default]
    Ansi,
    /// Plain text (strips ANSI codes)
    Plain,
    /// JSON with blocks structure
    Json,
}

/// Run the debug render command
pub async fn run(input_file: Option<String>, format: OutputFormat) -> Result<()> {
    // Read input JSON
    let json = if let Some(path) = input_file {
        std::fs::read_to_string(&path).context(format!("Failed to read input file: {}", path))?
    } else {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .context("Failed to read from stdin")?;
        buffer
    };

    let debug_input = DebugInput::from_json(&json)?;
    let state = debug_input.to_state();

    match format {
        OutputFormat::Json => {
            // Output the derived blocks as JSON
            let blocks = Blocks::from_state(&state);
            println!(
                "{}",
                serde_json::to_string_pretty(&blocks_to_json(&blocks))?
            );
        }
        OutputFormat::Plain | OutputFormat::Ansi => {
            // Render to a test backend
            let backend = TestBackend::new(debug_input.width, debug_input.height);
            let mut terminal = Terminal::new(backend)?;

            // Load default theme
            let settings = atuin_client::settings::Settings::new()?;
            let mut theme_manager = atuin_client::theme::ThemeManager::new(None, None);
            let theme = theme_manager.load_theme(&settings.theme.name, None);

            let ctx = RenderContext {
                theme,
                anchor_col: 0,
                textarea: Some(&state.textarea),
                max_height: debug_input.height,
            };

            terminal.draw(|frame| {
                render(frame, &state, &ctx);
            })?;

            // Get buffer content
            let buffer = terminal.backend().buffer();
            let output = buffer_to_string(buffer, matches!(format, OutputFormat::Plain));
            print!("{}", output);
        }
    }

    Ok(())
}

/// Convert blocks to JSON for debugging
fn blocks_to_json(blocks: &Blocks) -> serde_json::Value {
    serde_json::json!({
        "count": blocks.items.len(),
        "blocks": blocks.items.iter().map(|block| {
            serde_json::json!({
                "separator_above": block.separator_above,
                "title": block.title,
                "content": block.content.iter().map(content_to_json).collect::<Vec<_>>()
            })
        }).collect::<Vec<_>>()
    })
}

fn content_to_json(content: &crate::tui::view_model::Content) -> serde_json::Value {
    use crate::tui::view_model::Content;
    match content {
        Content::Input {
            text,
            active,
            cursor_pos,
        } => serde_json::json!({
            "type": "Input",
            "text": text,
            "active": active,
            "cursor_pos": cursor_pos
        }),
        Content::Command { text, faded } => serde_json::json!({
            "type": "Command",
            "text": text,
            "faded": faded
        }),
        Content::Text { markdown } => serde_json::json!({
            "type": "Text",
            "markdown": markdown
        }),
        Content::Error { message } => serde_json::json!({
            "type": "Error",
            "message": message
        }),
        Content::Warning {
            kind,
            text,
            pending_confirm,
        } => serde_json::json!({
            "type": "Warning",
            "kind": format!("{:?}", kind),
            "text": text,
            "pending_confirm": pending_confirm
        }),
        Content::Spinner { frame, status_text } => serde_json::json!({
            "type": "Spinner",
            "frame": frame,
            "status_text": status_text
        }),
        Content::ToolStatus {
            completed_count,
            current_label,
            frame,
        } => serde_json::json!({
            "type": "ToolStatus",
            "completed_count": completed_count,
            "current_label": current_label,
            "frame": frame
        }),
    }
}

/// Convert ratatui buffer to string
fn buffer_to_string(buffer: &ratatui::buffer::Buffer, strip_ansi: bool) -> String {
    let area = buffer.area;
    let mut output = String::new();

    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            if strip_ansi {
                output.push_str(cell.symbol());
            } else {
                // Include ANSI styling
                let fg = cell.fg;
                let bg = cell.bg;
                let mods = cell.modifier;

                // Simple ANSI encoding
                if fg != ratatui::style::Color::Reset
                    || bg != ratatui::style::Color::Reset
                    || !mods.is_empty()
                {
                    output.push_str("\x1b[");
                    let mut first = true;

                    if mods.contains(ratatui::style::Modifier::BOLD) {
                        output.push('1');
                        first = false;
                    }
                    if mods.contains(ratatui::style::Modifier::DIM) {
                        if !first {
                            output.push(';');
                        }
                        output.push('2');
                        first = false;
                    }
                    if mods.contains(ratatui::style::Modifier::REVERSED) {
                        if !first {
                            output.push(';');
                        }
                        output.push('7');
                        first = false;
                    }
                    if mods.contains(ratatui::style::Modifier::UNDERLINED) {
                        if !first {
                            output.push(';');
                        }
                        output.push('4');
                        first = false;
                    }

                    if let Some(code) = color_to_ansi(fg, true) {
                        if !first {
                            output.push(';');
                        }
                        output.push_str(&code);
                        first = false;
                    }

                    if let Some(code) = color_to_ansi(bg, false) {
                        if !first {
                            output.push(';');
                        }
                        output.push_str(&code);
                    }

                    output.push('m');
                }

                output.push_str(cell.symbol());

                if fg != ratatui::style::Color::Reset
                    || bg != ratatui::style::Color::Reset
                    || !mods.is_empty()
                {
                    output.push_str("\x1b[0m");
                }
            }
        }
        output.push('\n');
    }

    output
}

fn color_to_ansi(color: ratatui::style::Color, foreground: bool) -> Option<String> {
    use ratatui::style::Color;
    let base = if foreground { 30 } else { 40 };

    match color {
        Color::Reset => None,
        Color::Black => Some((base).to_string()),
        Color::Red => Some((base + 1).to_string()),
        Color::Green => Some((base + 2).to_string()),
        Color::Yellow => Some((base + 3).to_string()),
        Color::Blue => Some((base + 4).to_string()),
        Color::Magenta => Some((base + 5).to_string()),
        Color::Cyan => Some((base + 6).to_string()),
        Color::Gray | Color::White => Some((base + 7).to_string()),
        Color::DarkGray => Some((base + 60).to_string()),
        Color::LightRed => Some((base + 61).to_string()),
        Color::LightGreen => Some((base + 62).to_string()),
        Color::LightYellow => Some((base + 63).to_string()),
        Color::LightBlue => Some((base + 64).to_string()),
        Color::LightMagenta => Some((base + 65).to_string()),
        Color::LightCyan => Some((base + 66).to_string()),
        Color::Indexed(i) => Some(format!("{}8;5;{}", if foreground { 3 } else { 4 }, i)),
        Color::Rgb(r, g, b) => Some(format!(
            "{}8;2;{};{};{}",
            if foreground { 3 } else { 4 },
            r,
            g,
            b
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_input() {
        let json = r#"{
            "events": [
                {"type": "user_message", "content": "list files"},
                {"type": "tool_call", "id": "123", "name": "suggest_command", "input": {"command": "ls -la"}}
            ],
            "mode": "Review"
        }"#;

        let input = DebugInput::from_json(json).unwrap();
        assert_eq!(input.events.len(), 2);
        assert_eq!(input.mode, "Review");

        let state = input.to_state();
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.mode, AppMode::Review);
    }

    #[test]
    fn test_parse_streaming_state() {
        let json = r#"{
            "events": [
                {"type": "user_message", "content": "explain flags"}
            ],
            "mode": "Streaming",
            "streaming_text": "The -l flag means..."
        }"#;

        let input = DebugInput::from_json(json).unwrap();
        let state = input.to_state();
        assert_eq!(state.mode, AppMode::Streaming);
        assert_eq!(state.streaming_text, "The -l flag means...");
    }
}
