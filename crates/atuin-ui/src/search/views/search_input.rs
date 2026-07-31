use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
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
        // When modal, reserve the chip's column on the left; the prompt + query
        // fill the rest. Non-modal renders prompt + query across the whole row.
        let rest = match self.mode {
            Some(mode) => {
                let [chip, rest] = Layout::horizontal([
                    Constraint::Length(MODE_INDICATOR_WIDTH),
                    Constraint::Min(0),
                ])
                .areas(area);
                ModeIndicator {
                    mode,
                    theme: self.theme,
                }
                .render(chip, buf);
                rest
            }
            None => area,
        };

        let line = Line::from(vec![
            Span::styled(self.prompt, self.theme.annotation),
            Span::styled(self.input.value(), self.theme.base),
        ]);
        Paragraph::new(line).style(self.theme.base).render(rest, buf);
    }
}

/// The vim mode chip: `| MODE | `, drawn before the prompt when the interface is
/// modal. Fixed [`MODE_INDICATOR_WIDTH`] columns wide, so the caller reserves its
/// column and offsets the cursor by it.
#[derive(Clone, Copy)]
struct ModeIndicator<'render> {
    mode: Mode,
    theme: &'render Theme,
}

impl Widget for ModeIndicator<'_> {
    #[instrument(level = "trace", name = "ModeIndicator::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = Line::from(vec![
            Span::styled("| ", self.theme.annotation),
            Span::styled(
                self.mode.label(),
                self.theme.annotation.add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | ", self.theme.annotation),
        ]);
        Paragraph::new(line).style(self.theme.base).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Mode;
    use rstest::rstest;

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

    #[rstest]
    #[case::plain(None, "grep", "> grep")]
    #[case::normal(Some(Mode::Normal { count: None }), "grep", "| NORMAL | > grep")]
    #[case::search(Some(Mode::Search), "ls", "| SEARCH | > ls")]
    fn renders_optional_chip_then_prompt_and_query(
        #[case] mode: Option<Mode>,
        #[case] query: &str,
        #[case] expected_prefix: &str,
    ) {
        let line = render_line(mode, query, 30);
        assert!(line.starts_with(expected_prefix), "got {line:?}");
    }

    #[rstest]
    #[case::normal(Mode::Normal { count: None }, "| NORMAL | ")]
    #[case::search(Mode::Search, "| SEARCH | ")]
    fn chip_fills_exactly_its_reserved_width(#[case] mode: Mode, #[case] chip: &str) {
        let line = render_line(Some(mode), "", 30);
        assert_eq!(&line[..MODE_INDICATOR_WIDTH as usize], chip);
    }
}
