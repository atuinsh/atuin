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

const MAX_LENGTH: usize = " > 123ms 59s ago".len();

impl<'a> StatefulWidget for HistoryList<'a> {
    type State = ListState;

    #[allow(clippy::cast_possible_truncation)] // we do not expect MAX_LENGTH or ago.len() to overflow a u16
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

        let mut current_height = 0;
        for (i, item) in self
            .history
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(end - start)
        {
            current_height += 1;
            let (mut x, y) = (list_area.left(), list_area.bottom() - current_height);

            // these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
            // Yes, this is a hack, but it makes me feel happy
            let slices = " > 1 2 3 4 5 6 7 8 9   ";
            let slice_index = i.checked_sub(state.selected).unwrap_or(10).min(10) * 2;
            let ptr = &slices[slice_index..slice_index + 3];
            (x, _) = buf.set_stringn(x, y, ptr, list_area.width as usize, Style::default());

            let status = Style::default().fg(if item.success() {
                Color::Green
            } else {
                Color::Red
            });
            let dur = duration(item);
            buf.set_stringn(x, y, &dur, (list_area.width - x) as usize, status);

            let ago = ago(item);
            x = list_area.left() + (MAX_LENGTH - ago.len()) as u16;
            let style = Style::default().fg(Color::Blue);
            (x, _) = buf.set_stringn(x, y, &ago, (list_area.width - x) as usize, style);

            let mut style = Style::default();
            if slice_index == 0 {
                style = style.fg(Color::Red).add_modifier(Modifier::BOLD);
            }
            for section in item.command.split_ascii_whitespace() {
                x += 1;
                (x, _) = buf.set_stringn(x, y, &section, (list_area.width - x) as usize, style);
            }
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

fn duration(h: &History) -> String {
    let duration = Duration::from_nanos(u64::try_from(h.duration).unwrap_or(0));
    format_duration(duration)
}

fn ago(h: &History) -> String {
    let ago = chrono::Utc::now() - h.timestamp;

    // Account for the chance that h.timestamp is "in the future"
    // This would mean that "ago" is negative, and the unwrap here
    // would fail.
    // If the timestamp would otherwise be in the future, display
    // the time ago as 0.
    let ago = ago.to_std().unwrap_or_default();
    format_duration(ago) + " ago"
}
