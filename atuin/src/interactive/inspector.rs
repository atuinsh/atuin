use std::time::Duration;

use atuin_client::{history::History, settings::Settings};
use crossterm::event::KeyEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::{Alignment, Backend, Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Styled},
    text::{Span, Text},
    widgets::{Block, Paragraph, StatefulWidget, Widget},
    Frame,
};
use time::OffsetDateTime;

use crate::utils::duration::format_duration;

use super::search::State;

pub fn draw_inspector<T: Backend>(f: &mut Frame<'_, T>, chunk: Rect, history: &History) {
    let layout = Layout::new()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 4)])
        .split(chunk);

    let command = Paragraph::new(Text::from(Span::styled(
        history.command.as_str(),
        Style::default(),
    )))
    .alignment(Alignment::Center);

    f.render_widget(command, layout[0]);
}

// I'm going to break this out more, but just starting to move things around before changing
// structure and making it nicer.
pub fn inspector_input(state: &mut State, settings: &Settings, input: &KeyEvent) {}
