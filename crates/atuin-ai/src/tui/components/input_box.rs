//! Bordered input box component for the AI TUI.
//!
//! Wraps tui-textarea's TextArea, which handles rendering, wrapping, cursor
//! positioning, and height measurement natively. The component configures the
//! TextArea's block (border + titles) and forwards events to it.
//!
//! On Enter, sends `AiTuiEvent::SubmitInput` via the context-provided channel.

use std::sync::{Arc, Mutex, mpsc};

use crossterm::event::KeyModifiers;
use eye_declare::{Canvas, Elements, EventResult, Hooks, component, element, props};
use ratatui::widgets::{Block, Borders, Padding};
use ratatui_core::{
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
#[props]
pub(crate) struct InputBox {
    /// Title shown in top-left border
    pub title: String,
    /// Right-side label in top border
    pub title_right: String,
    /// Footer text shown in bottom border (keybinding hints)
    pub footer: String,
    /// Whether the input is currently active (shows cursor, accepts input)
    pub active: bool,
}

pub(crate) struct InputBoxState {
    textarea: Arc<Mutex<TextArea<'static>>>,
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
            textarea: Arc::new(Mutex::new(textarea)),
            tx: None,
        }
    }
}

fn make_block(props: &InputBox) -> Block<'static> {
    let border_style = Style::default().fg(Color::DarkGray);
    let title_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .padding(Padding::horizontal(1));

    if !props.title.is_empty() {
        block =
            block.title_top(Line::styled(format!(" {} ", props.title), title_style).left_aligned());
    }
    if !props.title_right.is_empty() {
        block = block.title_top(
            Line::styled(format!(" {} ", props.title_right), border_style).right_aligned(),
        );
    }
    if !props.footer.is_empty() {
        block = block.title_bottom(
            Line::styled(format!(" {} ", props.footer), border_style).right_aligned(),
        );
    }

    block
}

#[component(props = InputBox, state = InputBoxState)]
fn input_box(
    props: &InputBox,
    state: &InputBoxState,
    hooks: &mut Hooks<InputBox, InputBoxState>,
) -> Elements {
    hooks.use_focusable(props.active);
    hooks.use_autofocus();

    hooks.use_context::<mpsc::Sender<AiTuiEvent>>(|tx, _, state| {
        state.tx = tx.cloned();
    });

    hooks.use_event(move |event, props, state| {
        let state = state.read();

        if !props.active {
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
                        if text.trim().is_empty() {
                            return EventResult::Ignored;
                        }

                        textarea.clear();

                        if let Some(ref tx) = state.tx {
                            let _ = tx.send(AiTuiEvent::SubmitInput(text));
                        }
                        return EventResult::Consumed;
                    }
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
    });

    let textarea = state.textarea.clone();
    let block = make_block(props);
    let active = props.active;
    element!(
        Canvas(render_fn: move |area, buf| {
            let mut area = area;

            if area.height < 3 || area.width < 4 {
                return;
            }

            let height = {
                // TextArea handles scrolling internally if content overflows.
                let inner = block.inner(Rect::new(0, 0, area.width, u16::MAX));
                let chrome = (u16::MAX).saturating_sub(inner.height);
                let content = textarea.lock().unwrap().measure(area.width - 4);
                chrome + content.preferred_rows
            };

            area.height = height.min(7);
            let inner = block.clone().inner(area);
            block.clone().render(area, buf);

            let mut textarea = textarea.lock().unwrap();
            if active {
                textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
                textarea.set_placeholder_text("Type a message...");
            } else {
                textarea.set_cursor_style(Style::default());
                textarea.set_placeholder_text("");
            }

            // Render textarea into the inner area
            textarea.render(inner, buf);
        })
    )
}
