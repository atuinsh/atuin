use std::time::Duration;

use super::duration::format_duration;
use super::engines::SearchEngine;
use atuin_client::{
    history::History,
    settings::{UiColumn, UiColumnType},
    theme::{Meaning, Theme},
};
use atuin_common::utils::Escapable as _;
use itertools::Itertools;
use ratatui::{
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

// Default prefix length for backwards compatibility (used by interactive.rs)
#[allow(clippy::cast_possible_truncation)] // we know that this is <65536 length
pub const PREFIX_LENGTH: u16 = " > 123ms 59s ago".len() as u16;

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

        // Render each configured column
        for column in self.columns {
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
                UiColumnType::Command => self.command(h),
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
        // Right-align duration within its column width, plus trailing space
        let display = format!("{formatted:>w$} ");
        self.draw(&display, style.into());
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

        // Format as "Xs ago" right-aligned within column width, plus trailing space
        let w = width as usize;
        let time_str = format!("{time} ago");
        let display = format!("{time_str:>w$} ");
        self.draw(&display, style.into());
    }

    fn command(&mut self, h: &History) {
        let mut style = self.theme.as_style(Meaning::Base);
        let mut row_highlighted = false;
        if !self.alternate_highlight && (self.y as usize + self.state.offset == self.state.selected)
        {
            row_highlighted = true;
            // if not applying alternative highlighting to the whole row, color the command
            style = self.theme.as_style(Meaning::AlertError);
            style.attributes.set(style::Attribute::Bold);
        }

        let highlight_indices = self.history_highlighter.get_highlight_indices(
            h.command
                .escape_control()
                .split_ascii_whitespace()
                .join(" ")
                .as_str(),
        );

        let mut pos = 0;
        for section in h.command.escape_control().split_ascii_whitespace() {
            if pos != 0 {
                self.draw(" ", style.into());
            }
            for ch in section.chars() {
                if self.x > self.list_area.width {
                    // Avoid attempting to draw a command section beyond the width
                    // of the list
                    return;
                }
                let mut style = style;
                if highlight_indices.contains(&pos) {
                    if row_highlighted {
                        // if the row is highlighted bold is not enough as the whole row is bold
                        // change the color too
                        style = self.theme.as_style(Meaning::AlertWarn);
                    }
                    style.attributes.set(style::Attribute::Bold);
                }
                let s = ch.to_string();
                self.draw(&s, style.into());
                pos += s.len();
            }
            pos += 1;
        }
    }

    /// Render the absolute datetime column (e.g., "2025-01-22 14:35")
    fn datetime(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        // Format: YYYY-MM-DD HH:MM
        let formatted = h
            .timestamp
            .format(
                &time::format_description::parse("[year]-[month]-[day] [hour]:[minute]")
                    .expect("valid format"),
            )
            .unwrap_or_else(|_| "????-??-?? ??:??".to_string());
        let w = width as usize;
        let display = format!("{formatted:w$} ");
        self.draw(&display, style.into());
    }

    /// Render the directory column (working directory, truncated)
    fn directory(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        let cwd = &h.cwd;
        let char_count = cwd.chars().count();
        // Truncate from the left with "..." if too long, plus trailing space
        // Use character count for comparison and skip for UTF-8 safety
        let display = if char_count > w && w >= 4 {
            let truncated: String = cwd.chars().skip(char_count - (w - 3)).collect();
            format!("...{truncated} ")
        } else {
            format!("{cwd:w$} ")
        };
        self.draw(&display, style.into());
    }

    /// Render the host column (just the hostname)
    fn host(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        // Database stores hostname as "hostname:username"
        let host = h.hostname.split(':').next().unwrap_or(&h.hostname);
        let char_count = host.chars().count();
        // Use character count for comparison and take for UTF-8 safety
        let display = if char_count > w && w >= 4 {
            let truncated: String = host.chars().take(w.saturating_sub(4)).collect();
            format!("{truncated}... ")
        } else {
            format!("{host:w$} ")
        };
        self.draw(&display, style.into());
    }

    /// Render the user column
    fn user(&mut self, h: &History, width: u16) {
        let style = self.theme.as_style(Meaning::Annotation);
        let w = width as usize;
        // Database stores hostname as "hostname:username"
        let user = h.hostname.split(':').nth(1).unwrap_or("");
        let char_count = user.chars().count();
        // Use character count for comparison and take for UTF-8 safety
        let display = if char_count > w && w >= 4 {
            let truncated: String = user.chars().take(w.saturating_sub(4)).collect();
            format!("{truncated}... ")
        } else {
            format!("{user:w$} ")
        };
        self.draw(&display, style.into());
    }

    /// Render the exit code column
    fn exit_code(&mut self, h: &History, width: u16) {
        let style = if h.success() {
            self.theme.as_style(Meaning::AlertInfo)
        } else {
            self.theme.as_style(Meaning::AlertError)
        };
        let w = width as usize;
        let display = format!("{:>w$} ", h.exit);
        self.draw(&display, style.into());
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
