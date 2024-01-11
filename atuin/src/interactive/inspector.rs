use std::time::Duration;

use atuin_client::{
    database::Database,
    history::{History, HistoryStats},
    settings::Settings,
};
use crossterm::event::KeyEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::{Alignment, Backend, Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Styled, Stylize},
    text::{Span, Text},
    widgets::{Block, Borders, Cell, Padding, Paragraph, Row, StatefulWidget, Table, Widget},
    Frame,
};
use time::OffsetDateTime;

use crate::utils::duration::format_duration;

use super::search::State;

pub fn draw_inspector(f: &mut Frame<'_>, chunk: Rect, history: &History, stats: HistoryStats) {
    let vert_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 5), Constraint::Ratio(4, 5)])
        .split(chunk);

    let commands = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 2),
            Constraint::Ratio(1, 4),
        ])
        .split(vert_layout[0]);

    let stats_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
        .split(vert_layout[1]);

    let command = Paragraph::new(history.command.clone()).block(
        Block::new()
            .borders(Borders::ALL)
            .title("Command")
            .padding(Padding::horizontal(1)),
    );

    let previous = Paragraph::new(
        stats
            .previous
            .map_or("No previous command".to_string(), |prev| prev.command),
    )
    .block(
        Block::new()
            .borders(Borders::ALL)
            .title("Previous command")
            .padding(Padding::horizontal(1)),
    );

    let next = Paragraph::new(
        stats
            .next
            .map_or("No next command".to_string(), |next| next.command),
    )
    .block(
        Block::new()
            .borders(Borders::ALL)
            .title("Next command")
            .padding(Padding::horizontal(1)),
    );

    let duration = Duration::from_nanos(history.duration as u64);

    let rows = [
        Row::new(vec!["Time".to_string(), history.timestamp.to_string()]),
        Row::new(vec![
            "Duration".to_string(),
            format!(
                "{}.{}s",
                duration.as_secs().to_string(),
                duration.subsec_nanos()
            ),
        ]),
        Row::new(vec!["Exit".to_string(), history.exit.to_string()]),
        Row::new(vec!["Directory".to_string(), history.cwd.to_string()]),
        Row::new(vec!["Session".to_string(), history.session.to_string()]),
        Row::new(vec!["Total runs".to_string(), stats.total.to_string()]),
    ];

    let widths = [Constraint::Ratio(2, 5), Constraint::Ratio(3, 5)];

    let table = Table::new(rows, widths).column_spacing(1).block(
        Block::default()
            .title(history.command.clone())
            .padding(Padding::vertical(1)),
    );

    f.render_widget(table, stats_layout[0]);
    f.render_widget(previous, commands[0]);
    f.render_widget(command, commands[1]);
    f.render_widget(next, commands[2]);
}

// I'm going to break this out more, but just starting to move things around before changing
// structure and making it nicer.
pub fn inspector_input(state: &mut State, settings: &Settings, input: &KeyEvent) {}
