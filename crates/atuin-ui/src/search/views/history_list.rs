use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use tracing::instrument;

use crate::models::{HistoryList, HistoryRow};
use crate::search::views::syntax::highlight;
use crate::theme::Theme;

/// Column widths for the right-aligned time and duration fields.
const TIME_WIDTH: u16 = 8;
const DURATION_WIDTH: u16 = 6;

/// Renders the visible window of the [`HistoryList`] — one [`HistoryRowView`]
/// per visible row. Rows outside the loaded window show a placeholder until
/// their fetch lands. Pure: selection/scroll live in the model.
#[derive(Clone, Copy)]
pub struct HistoryListView<'render> {
    pub model: &'render HistoryList,
    pub theme: &'render Theme,
    /// Current time (unix seconds), for the relative "… ago" column.
    pub now: i64,
}

impl Widget for HistoryListView<'_> {
    #[instrument(level = "trace", name = "HistoryListView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        for i in 0..area.height {
            let index = self.model.offset() + i as usize;
            if index >= self.model.total() {
                break;
            }
            let line = Rect {
                y: area.y + i,
                height: 1,
                ..area
            };

            match self.model.row(index) {
                Some(row) => HistoryRowView {
                    theme: self.theme,
                    row,
                    now: self.now,
                    selected: index == self.model.selected(),
                }
                .render(line, buf),
                None => {
                    Paragraph::new(Span::styled("…", self.theme.annotation))
                        .style(self.theme.base)
                        .render(line, buf);
                }
            }
        }
    }
}

/// Renders a single history row: right-aligned relative time and duration, then
/// the syntax-highlighted command. The selected row is reversed (and its command
/// left un-highlighted, so the reverse-video reads cleanly).
#[derive(Clone, Copy)]
pub struct HistoryRowView<'render> {
    pub theme: &'render Theme,
    pub row: &'render HistoryRow,
    pub now: i64,
    pub selected: bool,
}

impl Widget for HistoryRowView<'_> {
    #[instrument(level = "trace", name = "HistoryRowView::render", skip_all, fields(area = ?area))]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let base = self.theme.base;
        let selected = base.add_modifier(Modifier::REVERSED);

        // Fill the row: the base background (if the theme sets one), or a solid
        // reverse-video bar when selected. The pieces render their fg on top.
        buf.set_style(area, if self.selected { selected } else { base });

        let [time_area, duration_area, command_area] = Layout::horizontal([
            Constraint::Length(TIME_WIDTH),
            Constraint::Length(DURATION_WIDTH),
            Constraint::Min(0),
        ])
        .spacing(1)
        .areas(area);

        // Relative time, right-aligned.
        let time_style = if self.selected { selected } else { self.theme.time };
        Paragraph::new(Span::styled(relative(self.now, self.row.timestamp), time_style))
            .alignment(Alignment::Right)
            .render(time_area, buf);

        // Duration, right-aligned, coloured by success.
        let duration_style = if self.selected {
            selected
        } else if self.row.exit == 0 {
            self.theme.duration_ok
        } else {
            self.theme.duration_err
        };
        Paragraph::new(Span::styled(duration(self.row.duration), duration_style))
            .alignment(Alignment::Right)
            .render(duration_area, buf);

        // Command: reverse-video when selected, else syntax-highlighted.
        let command = if self.selected {
            Line::from(Span::styled(self.row.command.as_str(), selected))
        } else {
            Line::from(highlight(&self.row.command, &self.theme.syntax, base))
        };
        Paragraph::new(command).render(command_area, buf);
    }
}

/// The largest non-zero time unit of `secs.subsec`, e.g. `"3d"`, `"1s"`,
/// `"814ms"`, `"0s"` — matching atuin's `largest_unit` display (truncating).
fn largest_unit(secs: u64, subsec_nanos: u32) -> String {
    const YEAR: u64 = 31_557_600;
    const MONTH: u64 = 2_630_016;
    const DAY: u64 = 86_400;
    const HOUR: u64 = 3_600;
    const MINUTE: u64 = 60;

    match secs {
        s if s >= YEAR => format!("{}y", s / YEAR),
        s if s >= MONTH => format!("{}mo", s / MONTH),
        s if s >= DAY => format!("{}d", s / DAY),
        s if s >= HOUR => format!("{}h", s / HOUR),
        s if s >= MINUTE => format!("{}m", s / MINUTE),
        s if s >= 1 => format!("{s}s"),
        _ => match subsec_nanos {
            n if n >= 1_000_000 => format!("{}ms", n / 1_000_000),
            n if n >= 1_000 => format!("{}us", n / 1_000),
            n if n >= 1 => format!("{n}ns"),
            _ => "0s".to_string(),
        },
    }
}

/// `"<largest-unit> ago"` for a command that ran at `timestamp` (unix seconds).
fn relative(now: i64, timestamp: i64) -> String {
    let elapsed = now.saturating_sub(timestamp).max(0) as u64;
    format!("{} ago", largest_unit(elapsed, 0))
}

/// Human duration from nanoseconds (negatives — "still running" — clamp to 0).
fn duration(nanos: i64) -> String {
    let nanos = nanos.max(0) as u64;
    largest_unit(nanos / 1_000_000_000, (nanos % 1_000_000_000) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largest_unit_picks_one_unit_and_truncates() {
        assert_eq!(largest_unit(0, 0), "0s");
        assert_eq!(largest_unit(0, 814_000_000), "814ms");
        assert_eq!(largest_unit(1, 500_000_000), "1s"); // truncates the .5
        assert_eq!(largest_unit(90, 0), "1m");
        assert_eq!(largest_unit(7_199, 0), "1h");
        assert_eq!(largest_unit(90_000, 0), "1d");
    }

    #[test]
    fn duration_and_relative_formats() {
        assert_eq!(duration(814_000_000), "814ms");
        assert_eq!(duration(-1), "0s"); // still running
        assert_eq!(relative(1_000, 985), "15s ago");
        assert_eq!(relative(1_000, 2_000), "0s ago"); // future clamps
    }
}
