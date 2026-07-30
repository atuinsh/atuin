use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use tracing::instrument;

use crate::models::SearchInput;
use crate::theme::Theme;

/// The search input line: a prompt followed by the current query. The terminal
/// cursor is positioned by the caller (the view knows `input.cursor_char()`).
#[derive(Clone, Copy)]
pub struct SearchInputView<'render> {
    pub prompt: &'render str,
    pub input: &'render SearchInput,
    pub theme: &'render Theme,
}

impl Widget for SearchInputView<'_> {
    #[instrument(level = "trace", name = "SearchInputView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = Line::from(vec![
            Span::styled(self.prompt, self.theme.annotation),
            Span::styled(self.input.value(), self.theme.base),
        ]);
        Paragraph::new(line).style(self.theme.base).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_text(buf: &Buffer, width: u16) -> String {
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[test]
    fn renders_prompt_and_query() {
        let theme = Theme::default();
        let mut input = SearchInput::new();
        for c in "grep".chars() {
            input.insert(c);
        }
        let view = SearchInputView {
            prompt: "> ",
            input: &input,
            theme: &theme,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        view.render(Rect::new(0, 0, 20, 1), &mut buf);
        assert!(buffer_text(&buf, 20).starts_with("> grep"));
    }
}
