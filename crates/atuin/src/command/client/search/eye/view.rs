//! Leaf elements for the eye-declare search UI.
//!
//! Both elements report a fixed height so the inline region never grows past
//! `inline_height`: a frame taller than the terminal would stream rows into
//! scrollback irreversibly, so the list clamps instead of sizing to content.

use atuin_client::history::History;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use eye_declare::Element;
use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::super::cursor::Cursor;

/// Bottom-anchored results: result 0 (best match) sits on the bottom row,
/// older/worse matches stack upward, matching the non-inverted atuin layout.
pub(super) struct ResultsList<'a> {
    pub results: &'a [History],
    pub selected: usize,
    pub rows: u16,
}

impl Element for ResultsList<'_> {
    fn height(&self, _width: u16) -> u16 {
        self.rows
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Scroll just enough to keep the selection in the window.
        let offset = self
            .selected
            .saturating_sub((area.height as usize).saturating_sub(1));
        for row in 0..area.height {
            let idx = offset + row as usize;
            let Some(h) = self.results.get(idx) else {
                break;
            };
            let style = if idx == self.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let y = area.y + area.height - 1 - row;
            let command = h.command.escape_non_printable();
            buf.set_stringn(area.x, y, &command, area.width as usize, style);
        }
    }
}

/// The search input line. Owns the hardware-cursor hint.
pub(super) struct InputLine<'a> {
    pub input: &'a Cursor,
}

const PROMPT: &str = "> ";
const PROMPT_WIDTH: u16 = 2;

impl Element for InputLine<'_> {
    fn height(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        buf.set_stringn(
            area.x,
            area.y,
            PROMPT,
            area.width as usize,
            Style::default(),
        );
        let text_x = area.x + PROMPT_WIDTH;
        if text_x < area.x + area.width {
            buf.set_stringn(
                text_x,
                area.y,
                self.input.as_str(),
                (area.width - PROMPT_WIDTH) as usize,
                Style::default(),
            );
        }
    }

    fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let col = PROMPT_WIDTH
            .saturating_add(u16::try_from(self.input.substring().width()).unwrap_or(u16::MAX));
        Some((col.min(area.width.saturating_sub(1)), 0))
    }
}
