//! The pieces that the pages of `atuin setup` draw with.

use atuin_common::string::wrap;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};

pub(super) const ACCENT: Color = Color::LightGreen;
pub(super) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// "atuin setup", then `after` on the same line.
pub(super) fn title(after: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let mut title = vec![Span::raw("  "), "atuin".fg(ACCENT).bold(), " setup".bold()];
    title.extend(after);
    vec![Line::default(), Line::from(title), Line::default()]
}

/// Whether a list is open, as the arrow left of its row's checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fold {
    Open,
    Closed,
}

/// What's left of a row's checkbox: the margin and room for a list's arrow.
pub(super) fn lead(at: bool, fold: Option<Fold>) -> [Span<'static>; 3] {
    let [arrow, gap] = fold_arrow(at, fold);
    [Span::raw("    "), arrow, gap]
}

/// A list's arrow, or room for one, and the gap after it. When the cursor is `at` the arrow, the
/// arrow is underlined to indicate that Enter acts on it.
pub(super) fn fold_arrow(at: bool, fold: Option<Fold>) -> [Span<'static>; 2] {
    let arrow = match fold {
        Some(Fold::Open) => "▾",
        Some(Fold::Closed) => "▸",
        None => return [Span::raw(" "), Span::raw(" ")],
    };
    target(at, Span::raw(arrow))
}

/// What Enter acts on, underlined when the cursor is `at` the item, and the gap after it.
pub(super) fn target(at: bool, mark: Span<'static>) -> [Span<'static>; 2] {
    let mark = if at {
        mark.underlined()
    } else {
        mark
    };
    [mark, Span::raw(" ")]
}

/// The button at the bottom of a screen, that does what it says.
pub(super) fn button(at: bool, label: &'static str) -> Line<'static> {
    let text = Span::raw(format!(" {label} "));
    let text = if at {
        text.fg(Color::Black).bg(ACCENT).bold()
    } else {
        text.fg(ACCENT)
    };
    let mut spans = lead(at, None).to_vec();
    spans.push(text);
    Line::from(spans)
}

/// The widest that wrapped text gets, so that the text fits in an 80-column terminal at the
/// deepest indent.
pub(super) const WRAP_COLUMNS: usize = 62;

/// `text`, wrapped to fit a screen `width` columns wide, each line `indent` columns in and in
/// `style`.
///
/// The lines leave the screen's last column free for a scrollbar.
pub(super) fn paragraph(text: &str, indent: usize, width: u16, style: Style) -> Vec<Line<'static>> {
    let columns = usize::from(width).saturating_sub(indent + 1).clamp(1, WRAP_COLUMNS);
    let indent = " ".repeat(indent);
    wrap(text, columns)
        .map(|line| {
            Line::from(vec![Span::raw(indent.clone()), Span::styled(line.to_owned(), style)])
        })
        .collect()
}

pub(super) fn dim(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into()).dim()
}

/// "enter toggle · ↑↓ move", the keys bold.
pub(super) fn hints(keys: &[(&'static str, &'static str)]) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(dim("   "));
        }
        spans.push(Span::raw(*key).bold());
        spans.push(dim(format!(" {what}")));
    }
    Line::from(spans)
}
