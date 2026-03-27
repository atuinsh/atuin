//! Bordered input box component for the AI TUI.
//!
//! Wraps tui-textarea's TextArea, which handles rendering, wrapping, cursor
//! positioning, and height measurement natively. The component configures the
//! TextArea's block (border + titles) and forwards events to it.
//!
//! On Enter, sends `AiTuiEvent::SubmitInput` via the context-provided channel.

use std::sync::{Mutex, mpsc};

use crossterm::event::KeyModifiers;
use eye_declare::{Component, EventResult, Hooks};
use ratatui::widgets::{Block, Borders, Padding};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Widget,
};
use tui_textarea::TextArea;

use crate::tui::events::AiTuiEvent;

/// A bordered text input box backed by tui-textarea.
///
/// Props configure the chrome (title, footer). The TextArea itself lives
/// in the component's State so it owns cursor, wrapping, and rendering.
#[derive(Default)]
pub struct InputBox {
    /// Title shown in top-left border
    pub title: String,
    /// Right-side label in top border
    pub title_right: String,
    /// Footer text shown in bottom border (keybinding hints)
    pub footer: String,
    /// Whether the input is currently active (shows cursor, accepts input)
    pub active: bool,
}

pub struct InputBoxState {
    textarea: Mutex<TextArea<'static>>,
    tx: Option<mpsc::Sender<AiTuiEvent>>,
}

impl Default for InputBoxState {
    fn default() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
        textarea.set_placeholder_text("Type a message...");
        textarea.set_placeholder_style(
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::DarkGray)
                .add_modifier(ratatui::style::Modifier::ITALIC),
        );
        Self {
            textarea: Mutex::new(textarea),
            tx: None,
        }
    }
}

impl InputBox {
    /// Build the ratatui Block with current titles/footer.
    fn make_block(&self) -> Block<'_> {
        let border_style = Style::default().fg(Color::DarkGray);
        let title_style = Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .padding(Padding::horizontal(1));

        if !self.title.is_empty() {
            block = block
                .title_top(Line::styled(format!(" {} ", self.title), title_style).left_aligned());
        }
        if !self.title_right.is_empty() {
            block = block.title_top(
                Line::styled(format!(" {} ", self.title_right), border_style).right_aligned(),
            );
        }
        if !self.footer.is_empty() {
            block = block.title_bottom(
                Line::styled(format!(" {} ", self.footer), border_style).right_aligned(),
            );
        }

        block
    }
}

impl Component for InputBox {
    type State = InputBoxState;

    fn initial_state(&self) -> Option<InputBoxState> {
        Some(InputBoxState::default())
    }

    fn lifecycle(&self, hooks: &mut Hooks<Self::State>, _state: &Self::State) {
        if self.active {
            hooks.use_autofocus();
        }
        hooks.use_context::<mpsc::Sender<AiTuiEvent>>(|tx, state| {
            state.tx = tx.cloned();
        });
    }

    fn render(&self, area: Rect, buf: &mut Buffer, state: &Self::State) {
        if area.height < 3 || area.width < 4 {
            return;
        }
        // Configure the block on each render so titles/footer stay current.
        // Note: set_block takes ownership, but the block is cheap to rebuild.
        // We can't call set_block here since we only have &self/&state,
        // so we render block + textarea separately.
        let block = self.make_block();
        let inner = block.inner(area);
        block.render(area, buf);

        let mut textarea = state.textarea.lock().unwrap();
        if self.active {
            textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
            textarea.set_placeholder_text("Type a message...");
        } else {
            textarea.set_cursor_style(Style::default());
            textarea.set_placeholder_text("");
        }

        // Render textarea into the inner area
        textarea.render(inner, buf);
    }

    fn desired_height(&self, width: u16, state: &Self::State) -> u16 {
        if width < 4 {
            return 3;
        }
        // TextArea handles scrolling internally if content overflows.
        let block = self.make_block();
        let inner = block.inner(Rect::new(0, 0, width, u16::MAX));
        let chrome = (u16::MAX).saturating_sub(inner.height);
        let content = state.textarea.lock().unwrap().measure(width - 4);
        chrome + content.preferred_rows
    }

    fn is_focusable(&self, _state: &Self::State) -> bool {
        self.active
    }

    fn handle_event(
        &self,
        event: &crossterm::event::Event,
        state: &mut Self::State,
    ) -> EventResult {
        if !self.active {
            return EventResult::Ignored;
        }

        if let crossterm::event::Event::Paste(text) = event {
            let mut textarea = state.textarea.lock().unwrap();
            textarea.insert_str(text);
            return EventResult::Consumed;
        }

        if let crossterm::event::Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return EventResult::Ignored;
            }

            // Let Ctrl+C bubble up to AtuinAi for exit handling
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == crossterm::event::KeyCode::Char('c')
            {
                return EventResult::Ignored;
            }

            let mut textarea = state.textarea.lock().unwrap();

            match key.code {
                crossterm::event::KeyCode::Char('j')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    textarea.insert_newline();
                    return EventResult::Consumed;
                }
                crossterm::event::KeyCode::Enter => {
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        textarea.insert_newline();
                        return EventResult::Consumed;
                    } else {
                        let text = textarea.lines().join("\n");
                        textarea.clear();

                        if text.trim().is_empty() {
                            return EventResult::Ignored;
                        }

                        if let Some(ref tx) = state.tx {
                            let _ = tx.send(AiTuiEvent::SubmitInput(text));
                        }
                        return EventResult::Consumed;
                    }
                }
                crossterm::event::KeyCode::Tab => {
                    return EventResult::Ignored;
                }
                // Esc: bubble up to app
                crossterm::event::KeyCode::Esc => {
                    return EventResult::Ignored;
                }
                _ => {}
            }

            // All other keys: forward to textarea.
            // tui-textarea can convert crossterm events itself.
            textarea.input(*key);

            if let Some(ref tx) = state.tx {
                let _ = tx.send(AiTuiEvent::InputUpdated(textarea.lines().join("\n")));
            }
            return EventResult::Consumed;
        }

        EventResult::Ignored
    }
}
