use std::time::Duration;

use super::duration::format_duration;
use super::engines::SearchEngine;
use atuin_client::{
    history::History,
    settings::{UiColumn, UiColumnType},
    theme::{Meaning, Theme},
};
use atuin_common::string::EllipsizeExt as _;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::string::Measure;
use atuin_common::string::align::Alignment;
use atuin_common::string::ellipsis::{Indicator, Pos};
use itertools::Itertools;
use ratatui::{
    backend::FromCrossterm,
    buffer::Buffer,
    crossterm::style,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, StatefulWidget, Widget},
};
use time::OffsetDateTime;

pub struct HistoryHighlighter<'a> {
    pub engine: &'a dyn SearchEngine,
    pub search_input: &'a str,
}

impl HistoryHighlighter<'_> {
    pub fn get_highlight_indices(&self, command: &str) -> Vec<usize> {
        self.engine
            .get_highlight_indices(command, self.search_input)
    }
}

pub struct HistoryList<'a> {
    history: &'a [History],
    block: Option<Block<'a>>,
    inverted: bool,
    /// Apply an alternative highlighting to the selected row
    alternate_highlight: bool,
    now: &'a dyn Fn() -> OffsetDateTime,
    indicator: &'a str,
    theme: &'a Theme,
    history_highlighter: HistoryHighlighter<'a>,
    show_numeric_shortcuts: bool,
    /// Columns to display (in order, after the indicator)
    columns: &'a [UiColumn],
}

#[derive(Default)]
pub struct ListState {
    offset: usize,
    selected: usize,
    max_entries: usize,
}

impl ListState {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }
}

impl StatefulWidget for HistoryList<'_> {
    type State = ListState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let list_area = self.block.take().map_or(area, |b| {
            let inner_area = b.inner(area);
            b.render(area, buf);
            inner_area
        });

        if list_area.width < 1 || list_area.height < 1 || self.history.is_empty() {
            return;
        }
        let list_height = list_area.height as usize;

        let (start, end) = self.get_items_bounds(state.selected, state.offset, list_height);
        state.offset = start;
        state.max_entries = end - start;

        let mut s = DrawState {
            buf,
            list_area,
            x: 0,
            y: 0,
            state,
            inverted: self.inverted,
            alternate_highlight: self.alternate_highlight,
            now: &self.now,
            indicator: self.indicator,
            theme: self.theme,
            history_highlighter: self.history_highlighter,
            show_numeric_shortcuts: self.show_numeric_shortcuts,
            columns: self.columns,
        };

        for item in self.history.iter().skip(state.offset).take(end - start) {
            s.render_row(item);

            // reset line
            s.y += 1;
            s.x = 0;
        }
    }
}

impl<'a> HistoryList<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        history: &'a [History],
        inverted: bool,
        alternate_highlight: bool,
        now: &'a dyn Fn() -> OffsetDateTime,
        indicator: &'a str,
        theme: &'a Theme,
        history_highlighter: HistoryHighlighter<'a>,
        show_numeric_shortcuts: bool,
        columns: &'a [UiColumn],
    ) -> Self {
        Self {
            history,
            block: None,
            inverted,
            alternate_highlight,
            now,
            indicator,
            theme,
            history_highlighter,
            show_numeric_shortcuts,
            columns,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    fn get_items_bounds(&self, selected: usize, offset: usize, height: usize) -> (usize, usize) {
        let offset = offset.min(self.history.len().saturating_sub(1));

        let max_scroll_space = height.min(10).min(self.history.len() - selected);
        if offset + height < selected + max_scroll_space {
            let end = selected + max_scroll_space;
            (end - height, end)
        } else if selected < offset {
            (selected, selected + height)
        } else {
            (offset, offset + height)
        }
    }
}

struct DrawState<'a> {
    buf: &'a mut Buffer,
    list_area: Rect,
    x: u16,
    y: u16,
    state: &'a ListState,
    inverted: bool,
    alternate_highlight: bool,
    now: &'a dyn Fn() -> OffsetDateTime,
    indicator: &'a str,
    theme: &'a Theme,
    history_highlighter: HistoryHighlighter<'a>,
    show_numeric_shortcuts: bool,
    columns: &'a [UiColumn],
}

// these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
// Yes, this is a hack, but it makes me feel happy
static SLICES: &str = " > 1 2 3 4 5 6 7 8 9   ";

impl DrawState<'_> {
    /// Render a complete row for a history item based on configured columns.
    fn render_row(&mut self, h: &History) {
        // Always render the indicator first (width 3)
        self.index();

        // Calculate the width for the expanding column
        // Fixed columns use their configured width + 1 (trailing space)
        let indicator_width: u16 = 3;
        let fixed_width: u16 = self
            .columns
            .iter()
            .filter(|c| !c.expand)
            .map(|c| c.width + 1)
            .sum();
        let expand_width = self
            .list_area
            .width
            .saturating_sub(indicator_width + fixed_width);

        let style = self.theme.as_style(Meaning::Base);
        // Render each configured column
        for (idx, column) in self.columns.iter().enumerate() {
            if idx != 0 {
                self.draw(" ", Style::from_crossterm(style));
            }
            let width = if column.expand {
                expand_width
            } else {
                column.width
            };
            match column.column_type {
                UiColumnType::Duration => self.duration(h, width),
                UiColumnType::Time => self.time(h, width),
                UiColumnType::Datetime => self.datetime(h, width),
                UiColumnType::Directory => self.directory(h, width),
                UiColumnType::Host => self.host(h, width),
                UiColumnType::User => self.user(h, width),
                UiColumnType::Exit => self.exit_code(h, width),
                UiColumnType::Command => self.command(h, width),
            }
        }
    }

    fn index(&mut self) {
        if !self.show_numeric_shortcuts {
            let i = self.y as usize + self.state.offset;
            let is_selected = i == self.state.selected();
            let prompt: &str = if is_selected { self.indicator } else { "   " };
            self.draw(prompt, Style::default());
            return;
        }

        // these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
        // Yes, this is a hack, but it makes me feel happy

        let i = self.y as usize + self.state.offset;
        let i = i.checked_sub(self.state.selected);
        let i = i.unwrap_or(10).min(10) * 2;
        let prompt: &str = if i == 0 {
            self.indicator
        } else {
            &SLICES[i..i + 3]
        };
        self.draw(prompt, Style::default());
    }

    fn duration(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(if h.success() {
            Meaning::AlertInfo
        } else {
            Meaning::AlertError
        });
        let duration = Duration::from_nanos(u64::try_from(h.duration).unwrap_or(0));
        let formatted = format_duration(duration);
        let w = width as usize;
        // Right-align within the column, ellipsizing if it somehow overflows.
        let display = formatted.pad_ellipsize(
            Measure::Columns(w),
            Pos::End,
            Indicator::UNICODE,
            Alignment::End,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    fn time(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Guidance);

        // Account for the chance that h.timestamp is "in the future"
        // This would mean that "since" is negative, and the unwrap here
        // would fail.
        // If the timestamp would otherwise be in the future, display
        // the time since as 0.
        let since = (self.now)() - h.timestamp;
        let time = format_duration(since.try_into().unwrap_or_default());

        // Format as "Xs ago" right-aligned within column width
        let w = width as usize;
        let time_str = format!("{time} ago");

        let display = time_str.pad_ellipsize(
            Measure::Columns(w),
            Pos::End,
            Indicator::UNICODE,
            Alignment::End,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    fn command(&mut self, h: &History, _width: u16) {
        let mut style = self.theme.as_style(Meaning::Base);
        let mut row_highlighted = false;
        if !self.alternate_highlight && (self.y as usize + self.state.offset == self.state.selected)
        {
            row_highlighted = true;
            // if not applying alternative highlighting to the whole row, color the command
            style = self.theme.as_style(Meaning::AlertError);
            style.attributes.set(style::Attribute::Bold);
        }

        // Build the normalized command string (whitespace-collapsed, control chars escaped)
        let normalized: String = h
            .command
            .escape_non_printable()
            .split_ascii_whitespace()
            .join(" ");

        let highlight_indices = self.history_highlighter.get_highlight_indices(&normalized);

        // Calculate the available width for the command text.
        // `self.x` is already past the indicator and any preceding columns,
        // so the remaining width is how far we can draw.
        let avail = (self.list_area.width.saturating_sub(self.x)) as usize;

        // Truncate long commands from the middle to show both start and end,
        // so users can identify commands even in narrow terminals (issue #3596).
        let ellipsized =
            normalized.ellipsize(Measure::Columns(avail), Pos::Middle, Indicator::UNICODE);
        let display = ellipsized.to_string();
        for (i, ch) in display.char_indices() {
            if self.x > self.list_area.width {
                return;
            }
            // Map each output cell back to its source byte and test the existing
            // highlight set; a cell on the spliced ellipsis maps to None and is
            // never highlighted (this is why the "…" is never bolded).
            let highlighted = ellipsized
                .source_index(i)
                .is_some_and(|b| highlight_indices.contains(&b));
            let mut char_style = style;
            if highlighted {
                if row_highlighted {
                    char_style = self.theme.as_style(Meaning::AlertWarn);
                }
                char_style.attributes.set(style::Attribute::Bold);
            }
            self.draw(&ch.to_string(), Style::from_crossterm(char_style));
        }
    }

    /// Render the absolute datetime column (e.g., "2025-01-22 14:35")
    fn datetime(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        // Format: YYYY-MM-DD HH:MM
        let formatted = h
            .timestamp
            .format(
                &time::format_description::parse_borrowed::<1>(
                    "[year]-[month]-[day] [hour]:[minute]",
                )
                .expect("valid format"),
            )
            .unwrap_or_else(|_| "????-??-?? ??:??".to_string());
        let w = width as usize;
        let display = formatted.pad_ellipsize(
            Measure::Columns(w),
            Pos::End,
            Indicator::UNICODE,
            Alignment::Start,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    /// Render the directory column (working directory, truncated)
    fn directory(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        let cwd = &h.cwd;
        // Elide from the left with "…" so the leaf directory stays visible;
        // pad to the column width when it already fits.
        let display = cwd.pad_ellipsize(
            Measure::Columns(w),
            Pos::Start,
            Indicator::UNICODE,
            Alignment::Start,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    /// Render the host column (just the hostname)
    fn host(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        // Database stores hostname as "hostname:username"
        let host = h.hostname.split(':').next().unwrap_or(&h.hostname);
        let display = host.pad_ellipsize(
            Measure::Columns(w),
            Pos::End,
            Indicator::UNICODE,
            Alignment::Start,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    /// Render the user column
    fn user(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        // Database stores hostname as "hostname:username"
        let user = h.hostname.split(':').nth(1).unwrap_or("");
        let display = user.pad_ellipsize(
            Measure::Columns(w),
            Pos::End,
            Indicator::UNICODE,
            Alignment::Start,
        );
        self.draw(&display, Style::from_crossterm(style));
    }

    /// Render the exit code column
    fn exit_code(&mut self, h: &History, width: u16) {
        let style = if h.success() {
            self.theme.as_style(Meaning::AlertInfo)
        } else {
            self.theme.as_style(Meaning::AlertError)
        };
        let w = width as usize;
        let display = format!("{:>w$}", h.exit);
        self.draw(&display, Style::from_crossterm(style));
    }

    fn draw(&mut self, s: &str, mut style: Style) {
        let cx = self.list_area.left() + self.x;

        let cy = if self.inverted {
            self.list_area.top() + self.y
        } else {
            self.list_area.bottom() - self.y - 1
        };

        if self.alternate_highlight && (self.y as usize + self.state.offset == self.state.selected)
        {
            style = style.add_modifier(Modifier::REVERSED);
        }

        let w = (self.list_area.width - self.x) as usize;
        self.x += self.buf.set_stringn(cx, cy, s, w, style).0 - cx;
    }
}
