use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use tracing::instrument;

use crate::models::Action;
use crate::theme::Theme;

/// Renders a single [`Action`] as a key hint in caret notation — bold combo,
/// then the label — over the theme's annotation style.
#[derive(Clone, Copy)]
pub struct KeyHintView<'render> {
    pub theme: &'render Theme,
    pub action: Action,
}

impl Widget for KeyHintView<'_> {
    #[instrument(level = "trace", name = "KeyHintView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let combo_style = self.theme.annotation.add_modifier(Modifier::BOLD);

        let mut spans = Vec::with_capacity(4);
        if self.action.ctrl {
            spans.push(Span::styled("^", combo_style));
        }
        spans.push(Span::styled(self.action.key, combo_style));
        spans.push(Span::raw(" "));
        spans.push(Span::raw(self.action.label));

        Paragraph::new(Line::from(spans))
            .style(self.theme.annotation)
            .render(area, buf);
    }
}
