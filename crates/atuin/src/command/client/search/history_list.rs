use std::time::Duration;

use atuin_client::{
    history::History,
    theme::{Meaning, Theme},
};
use atuin_common::utils::Escapable as _;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, StatefulWidget, Widget},
};
use time::OffsetDateTime;

use super::duration::format_duration;

pub struct HistoryList<'a> {
    history: &'a [History],
    block: Option<Block<'a>>,
    inverted: bool,
    /// Apply an alternative highlighting to the selected row
    alternate_highlight: bool,
    now: &'a dyn Fn() -> OffsetDateTime,
    theme: &'a Theme,
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

    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }
}

impl<'a> StatefulWidget for HistoryList<'a> {
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
            theme: self.theme,
        };

        for item in self.history.iter().skip(state.offset).take(end - start) {
            s.index();
            s.duration(item);
            s.time(item);
            s.command(item);

            // reset line
            s.y += 1;
            s.x = 0;
        }
    }
}

impl<'a> HistoryList<'a> {
    pub fn new(
        history: &'a [History],
        inverted: bool,
        alternate_highlight: bool,
        now: &'a dyn Fn() -> OffsetDateTime,
        theme: &'a Theme,
    ) -> Self {
        Self {
            history,
            block: None,
            inverted,
            alternate_highlight,
            now,
            theme,
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
    theme: &'a Theme,
}

// longest line prefix I could come up with
#[allow(clippy::cast_possible_truncation)] // we know that this is <65536 length
pub const PREFIX_LENGTH: u16 = " > 123ms 59s ago".len() as u16;
static SPACES: &str = "                ";
static _ASSERT: () = assert!(SPACES.len() == PREFIX_LENGTH as usize);

impl DrawState<'_> {
    fn index(&mut self) {
        // these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
        // Yes, this is a hack, but it makes me feel happy
        static SLICES: &str = " > 1 2 3 4 5 6 7 8 9   ";

        let i = self.y as usize + self.state.offset;
        let i = i.checked_sub(self.state.selected);
        let i = i.unwrap_or(10).min(10) * 2;
        self.draw(&SLICES[i..i + 3], Style::default());
    }

    fn duration(&mut self, h: &History) {
        let status = self.theme.as_style(if h.success() {
            Meaning::AlertInfo
        } else {
            Meaning::AlertError
        });
        let duration = Duration::from_nanos(u64::try_from(h.duration).unwrap_or(0));
        self.draw(&format_duration(duration), status.into());
    }

    #[allow(clippy::cast_possible_truncation)] // we know that time.len() will be <6
    fn time(&mut self, h: &History) {
        let style = self.theme.as_style(Meaning::Guidance);

        // Account for the chance that h.timestamp is "in the future"
        // This would mean that "since" is negative, and the unwrap here
        // would fail.
        // If the timestamp would otherwise be in the future, display
        // the time since as 0.
        let since = (self.now)() - h.timestamp;
        let time = format_duration(since.try_into().unwrap_or_default());

        // pad the time a little bit before we write. this aligns things nicely
        // skip padding if for some reason it is already too long to align nicely
        let padding =
            usize::from(PREFIX_LENGTH).saturating_sub(usize::from(self.x) + 4 + time.len());
        self.draw(&SPACES[..padding], Style::default());

        self.draw(&time, style.into());
        self.draw(" ago", style.into());
    }

    fn command(&mut self, h: &History) {
        let mut style = self.theme.as_style(Meaning::Base);
        if !self.alternate_highlight && (self.y as usize + self.state.offset == self.state.selected)
        {
            // if not applying alternative highlighting to the whole row, color the command
            style = self.theme.as_style(Meaning::AlertError);
            style.attributes.set(crossterm::style::Attribute::Bold);
        }

        for section in h.command.escape_control().split_ascii_whitespace() {
            self.draw(" ", style.into());
            if self.x > self.list_area.width {
                // Avoid attempting to draw a command section beyond the width
                // of the list
                return;
            }
            self.draw(section, style.into());
        }
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
