use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use tracing::instrument;

use crate::models::{Mode, SearchInput};
use crate::theme::Theme;

/// Columns the `| MODE | ` indicator occupies. Constant because both mode labels
/// ("NORMAL"/"SEARCH") are six columns wide: `"| "` (2) + label (6) + `" | "` (3).
pub const MODE_INDICATOR_WIDTH: u16 = 11;

/// The search input line: an optional `| MODE | ` indicator, a prompt, then the
/// current query. The terminal cursor is positioned by the caller (the view knows
/// `input.cursor_char()` and [`MODE_INDICATOR_WIDTH`]).
#[derive(Clone, Copy)]
pub struct SearchInputView<'render> {
    pub prompt: &'render str,
    pub input: &'render SearchInput,
    pub theme: &'render Theme,
    /// The current vim mode, or `None` for the plain interface. `Some` draws a
    /// `| MODE | ` chip before the prompt.
    pub mode: Option<Mode>,
}

impl Widget for SearchInputView<'_> {
    #[instrument(level = "trace", name = "SearchInputView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = Vec::with_capacity(5);
        if let Some(mode) = self.mode {
            spans.push(Span::styled("| ", self.theme.annotation));
            spans.push(Span::styled(
                mode.label(),
                self.theme.annotation.add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" | ", self.theme.annotation));
        }
        spans.push(Span::styled(self.prompt, self.theme.annotation));
        spans.push(Span::styled(self.input.value(), self.theme.base));
        Paragraph::new(Line::from(spans))
            .style(self.theme.base)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Mode;

    fn render_line(mode: Option<Mode>, query: &str, width: u16) -> String {
        let theme = Theme::default();
        let mut input = SearchInput::new();
        for c in query.chars() {
            input.insert(c);
        }
        let view = SearchInputView {
            prompt: "> ",
            input: &input,
            theme: &theme,
            mode,
        };
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[test]
    fn plain_mode_has_no_indicator() {
        assert!(render_line(None, "grep", 30).starts_with("> grep"));
    }

    #[test]
    fn normal_mode_shows_indicator() {
        let line = render_line(Some(Mode::Normal { count: None }), "grep", 30);
        assert!(line.starts_with("| NORMAL | > grep"), "got {line:?}");
    }

    #[test]
    fn search_mode_shows_indicator() {
        let line = render_line(Some(Mode::Search), "ls", 30);
        assert!(line.starts_with("| SEARCH | > ls"), "got {line:?}");
    }

    #[test]
    fn indicator_width_matches_rendered_prefix() {
        let line = render_line(Some(Mode::Normal { count: None }), "", 30);
        assert_eq!(&line[..MODE_INDICATOR_WIDTH as usize], "| NORMAL | ");
    }
}
