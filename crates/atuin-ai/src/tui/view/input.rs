//! The input area: tui-textarea wrapped as an eye-declare element, plus the
//! panel chrome, contextual hint line, and slash-command suggestions.
//!
//! tui-textarea's `TextArea` lives in the model as a plain value (strict
//! Elm); the `RefCell` exists only because `measure()` memoizes its wrap
//! layout behind `&mut self` while `Element::height` takes `&self`. Update
//! code uses `get_mut()` (compile-time borrow); only this adapter pays the
//! runtime borrow, and `height`/`render` never overlap.

use std::cell::RefCell;

use eye_declare::{AnyElement, Element, ElementExt, Fluent, col, panel, text};
use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::widgets::Widget;
use tui_textarea::TextArea;

use crate::tui::slash::SlashCommandSearchResult;

/// Content rows the editor grows to before scrolling internally.
const MAX_INPUT_ROWS: u16 = 5;

/// Build the editor with atuin-ai's configuration. Chrome (border/titles)
/// comes from the surrounding `panel`, not a tui-textarea block.
pub(crate) fn new_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_cursor_line_style(Style::default());
    textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
    textarea.set_placeholder_text("Type a message...");
    textarea.set_placeholder_style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    );
    textarea.set_max_rows(MAX_INPUT_ROWS);
    textarea
}

/// tui-textarea as an `Element`: measure + render, nothing else. Editing
/// happens in `update` via keymap fallthrough.
struct InputEditor<'a> {
    textarea: &'a RefCell<TextArea<'static>>,
    active: bool,
}

impl Element for InputEditor<'_> {
    fn height(&self, width: u16) -> u16 {
        self.textarea.borrow_mut().measure(width).preferred_rows
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut textarea = self.textarea.borrow_mut();
        if self.active {
            textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
            textarea.set_placeholder_text("Type a message...");
        } else {
            textarea.set_cursor_style(Style::default());
            textarea.set_placeholder_text("");
        }
        (&*textarea).render(area, buf);
    }
}

/// The full input area: bordered editor, contextual hint line, and slash
/// suggestions.
pub(crate) fn input_area<'a>(
    textarea: &'a RefCell<TextArea<'static>>,
    active: bool,
    footer: &str,
    show_command_hint: bool,
    slash_results: &[SlashCommandSearchResult],
) -> AnyElement<'a> {
    let border_style = Style::default().fg(Color::DarkGray);
    let title_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);

    col()
        .child(
            panel(InputEditor { textarea, active })
                .title("Generate a command or ask a question")
                .title_right("Atuin AI")
                .footer(footer)
                .border_style(border_style)
                .title_style(title_style)
                .pad_x(1),
        )
        .when(show_command_hint, |c| {
            c.child(
                text("[Enter] Execute suggested command  [Tab] Insert Command")
                    .style(Style::default().fg(Color::Gray)),
            )
        })
        .children(
            slash_results
                .iter()
                .take(4)
                .enumerate()
                .map(|(i, result)| slash_row(result, i == 0)),
        )
        .pad_top(1)
        .any()
}

fn slash_row(result: &SlashCommandSearchResult, first: bool) -> AnyElement<'static> {
    let name = &result.command.name;
    let (start, end) = result.span;
    let blue = Style::default().fg(Color::Blue);

    text(format!("/{}", &name[..start]))
        .style(blue)
        .span(&name[start..end], blue.add_modifier(Modifier::UNDERLINED))
        .span(&name[end..], blue)
        .span(" - ", Style::default())
        .span(&result.command.description, Style::default())
        .when(first, |t| {
            t.span(
                " [Tab] Insert",
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC)
                    .add_modifier(Modifier::DIM),
            )
        })
        .any()
}
