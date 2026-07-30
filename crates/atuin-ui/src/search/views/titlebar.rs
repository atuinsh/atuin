use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Modifier,
    text::Span,
    widgets::{Block, Paragraph, StatefulWidget, Widget},
};
use ratatui_image::{Resize, StatefulImage, protocol::StatefulProtocol};
use tracing::instrument;

use crate::models::{ActionCtx, Model};
use crate::search::views::key_hint::KeyHintView;
use crate::theme::Theme;

/// Title strings, concatenated at compile time so rendering never allocates.
const TITLE: &str = concat!("Atuin v", env!("CARGO_PKG_VERSION"));
const TITLE_UPDATE: &str = concat!("Atuin v", env!("CARGO_PKG_VERSION"), " - UPDATE");

/// Columns reserved at the left of the header for the turtle logo. Kept small —
/// graphics protocols (kitty, iTerm2, sixel) render sub-cell, so a one-row
/// header fits the turtle fine.
const LOGO_WIDTH: u16 = 2;

/// The title bar (header): a solid banner with the turtle logo on the left and,
/// to its right, the `Atuin vX.Y.Z` title, the centered shortcut hints (reduced
/// from the model's [`ActionCtx`]), and the right-aligned history count.
///
/// Renders from the [`Model`]; the logo is genuine state (its encoded image is
/// cached per size), so it is threaded in as [`Self::State`].
#[derive(Clone, Copy)]
pub struct TitleBarView<'render> {
    pub model: &'render Model,
    /// Show the `- UPDATE` title variant, styled with the error style.
    pub update_available: bool,
    /// History count; `None` renders nothing (e.g. still loading).
    pub count: Option<u64>,
}

impl StatefulWidget for TitleBarView<'_> {
    /// The turtle logo's render state, or `None` when the terminal can't show
    /// images. Persists across frames — the encoded image is cached per size.
    type State = Option<StatefulProtocol>;

    #[instrument(level = "trace", name = "TitleBarView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer, logo: &mut Self::State) {
        let theme = &self.model.theme;

        // Uniform banner background; the pieces render on top.
        Block::default().style(theme.base).render(area, buf);

        // Reserve a left column for the logo; the rest holds the header content.
        let [logo_area, rest] =
            Layout::horizontal([Constraint::Length(LOGO_WIDTH), Constraint::Min(0)]).areas(area);

        if let Some(protocol) = logo {
            StatefulImage::<StatefulProtocol>::default()
                .resize(Resize::Fit(None))
                .render(logo_area, buf, protocol);
        }

        // Header content on a single row, vertically centered beside the logo.
        let content = Rect {
            y: rest.y + rest.height.saturating_sub(1) / 2,
            height: 1,
            ..rest
        };
        let [title_area, help_area, count_area] = Layout::horizontal([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(3, 5),
            Constraint::Ratio(1, 5),
        ])
        .areas(content);

        Title {
            theme,
            update_available: self.update_available,
        }
        .render(title_area, buf);

        Shortcuts {
            theme,
            actions: self.model.ctx(),
        }
        .render(help_area, buf);

        HistoryCount {
            theme,
            count: self.count,
        }
        .render(count_area, buf);
    }
}

/// Left-aligned, bold `Atuin vX.Y.Z`, filled with the resolved style so a
/// themed background paints a solid banner.
#[derive(Clone, Copy)]
struct Title<'render> {
    theme: &'render Theme,
    update_available: bool,
}

impl Widget for Title<'_> {
    #[instrument(level = "trace", name = "Title::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (text, style) = if self.update_available {
            (TITLE_UPDATE, self.theme.error)
        } else {
            (TITLE, self.theme.base)
        };

        Paragraph::new(Span::styled(text, style.add_modifier(Modifier::BOLD)))
            .style(style)
            .alignment(Alignment::Left)
            .render(area, buf);
    }
}

/// Centered row of the currently-available actions, each rendered as a key hint.
#[derive(Clone, Copy)]
struct Shortcuts<'render> {
    theme: &'render Theme,
    actions: ActionCtx<'render>,
}

impl Widget for Shortcuts<'_> {
    #[instrument(level = "trace", name = "Shortcuts::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let slots = Layout::horizontal(
            self.actions
                .actions()
                .map(|action| Constraint::Length(action.width())),
        )
        .flex(Flex::Center)
        .spacing(2)
        .split(area);

        for (action, slot) in self.actions.actions().zip(slots.iter()) {
            KeyHintView {
                theme: self.theme,
                action,
            }
            .render(*slot, buf);
        }
    }
}

/// Right-aligned `count: N`, or nothing until the count is known.
#[derive(Clone, Copy)]
struct HistoryCount<'render> {
    theme: &'render Theme,
    count: Option<u64>,
}

impl Widget for HistoryCount<'_> {
    #[instrument(level = "trace", name = "HistoryCount::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Trailing space keeps the count off the right edge.
        let text = self
            .count
            .map_or_else(String::new, |count| format!("count: {count} "));

        Paragraph::new(Span::raw(text))
            .style(self.theme.annotation)
            .alignment(Alignment::Right)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{HistoryList, SearchInput};
    use ratatui_image::picker::Picker;

    const TURTLE_PNG: &[u8] = include_bytes!("../../../assets/atuin-turtle.png");

    fn render_header(width: u16, height: u16, logo: &mut Option<StatefulProtocol>) -> Buffer {
        let model = Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
        };
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        TitleBarView {
            model: &model,
            update_available: false,
            count: Some(42),
        }
        .render(area, &mut buf, logo);
        buf
    }

    fn buffer_text(buf: &Buffer, width: u16, height: u16) -> String {
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        text
    }

    #[test]
    fn renders_header_text() {
        let buf = render_header(80, 3, &mut None);
        let text = buffer_text(&buf, 80, 3);
        assert!(text.contains("Atuin v"), "title missing: {text:?}");
        assert!(text.contains("count: 42"), "count missing: {text:?}");
        assert!(text.contains("run"), "shortcut missing: {text:?}");
    }

    #[test]
    fn renders_turtle_logo() {
        let picker = Picker::halfblocks();
        let image = image::load_from_memory(TURTLE_PNG).expect("decode turtle png");
        let mut logo = Some(picker.new_resize_protocol(image));

        let buf = render_header(80, 3, &mut logo);

        // The reserved logo column should contain rendered (non-space) cells.
        let logo_filled =
            (0..3).any(|y| (0..LOGO_WIDTH).any(|x| buf[(x, y)].symbol().trim() != ""));
        assert!(logo_filled, "logo area is empty");
    }
}
