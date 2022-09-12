use std::time::Duration;

use atuin_client::history::History;
use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, StatefulWidget, Widget},
};

use super::format_duration;

pub struct HistoryList<'a> {
    history: &'a [History],
    block: Option<Block<'a>>,
    /// Style used as a base style for the widget
    style: Style,
}

#[derive(Default)]
pub struct ListState {
    offset: usize,
    selected: usize,
}

impl ListState {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }
}

impl<'a> StatefulWidget for HistoryList<'a> {
    type State = ListState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        buf.set_style(area, self.style);
        let list_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

        if list_area.width < 1 || list_area.height < 1 {
            return;
        }

        if self.history.is_empty() {
            return;
        }
        let list_height = list_area.height as usize;

        let (start, end) = self.get_items_bounds(state.selected, state.offset, list_height);
        state.offset = start;

        let mut y = list_area.bottom();
        for (i, item) in self
            .history
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(end - start)
        {
            y += 1;
            let mut s = DrawState {
                buf,
                width: list_area.width as usize,
                x: list_area.left(),
                y,
                i,
                state,
            };

            s.index();
            s.duration(item);
            s.time(list_area.left(), item);
            s.command(item);
        }
    }
}

impl<'a> HistoryList<'a> {
    pub fn new(history: &'a [History]) -> Self {
        Self {
            history,
            block: None,
            style: Style::default(),
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    fn get_items_bounds(&self, selected: usize, offset: usize, height: usize) -> (usize, usize) {
        let offset = offset.min(self.history.len().saturating_sub(1));

        if offset + height < selected + 10 {
            (selected + 10 - height, selected + 10)
        } else if selected < offset {
            (selected, selected + height)
        } else {
            (offset, offset + height)
        }
    }
}

struct DrawState<'a> {
    buf: &'a mut Buffer,
    width: usize,
    x: u16,
    y: u16,
    i: usize,
    state: &'a ListState,
}

// longest line prefix I could come up with
const PREFIX_LENGTH: usize = " > 123ms 59s ago".len();

impl DrawState<'_> {
    fn index(&mut self) {
        // these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
        // Yes, this is a hack, but it makes me feel happy
        static SLICES: &str = " > 1 2 3 4 5 6 7 8 9   ";
        let i = self.i.checked_sub(self.state.selected);
        let i = i.unwrap_or(10).min(10) * 2;
        self.draw(&SLICES[i..i + 3], Style::default());
    }

    fn duration(&mut self, h: &History) {
        let status = Style::default().fg(if h.success() {
            Color::Green
        } else {
            Color::Red
        });
        let dur = duration(h);
        self.draw(&dur, status);
    }

    #[allow(clippy::cast_possible_truncation)] // we do not expect PREFIX_LENGTH or ago.len() to overflow a u16
    fn time(&mut self, left: u16, h: &History) {
        let status = Style::default().fg(Color::Blue);
        let time = time(h);

        // pad the time a little bit before we write. this aligns things nicely
        let x = left + (PREFIX_LENGTH - time.len()) as u16;
        self.width -= (x - self.x) as usize;
        self.x = x;

        self.draw(&time, status);
    }

    fn command(&mut self, h: &History) {
        let mut style = Style::default();
        if self.i == self.state.selected {
            style = style.fg(Color::Red).add_modifier(Modifier::BOLD);
        }

        for section in h.command.split_ascii_whitespace() {
            self.x += 1;
            self.width -= 1;
            self.draw(section, style);
        }
    }

    fn draw(&mut self, s: &str, style: Style) {
        let x = self.buf.set_stringn(self.x, self.y, s, self.width, style).0;
        self.width -= (x - self.x) as usize;
        self.x = x;
    }
}

fn duration(h: &History) -> String {
    let duration = Duration::from_nanos(u64::try_from(h.duration).unwrap_or(0));
    format_duration(duration)
}

fn time(h: &History) -> String {
    let ago = chrono::Utc::now() - h.timestamp;

    // Account for the chance that h.timestamp is "in the future"
    // This would mean that "ago" is negative, and the unwrap here
    // would fail.
    // If the timestamp would otherwise be in the future, display
    // the time ago as 0.
    let ago = ago.to_std().unwrap_or_default();
    format_duration(ago) + " ago"
}
