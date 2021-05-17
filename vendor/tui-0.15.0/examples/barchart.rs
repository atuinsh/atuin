#[allow(dead_code)]
mod util;

use crate::util::event::{Event, Events};
use std::{error::Error, io};
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{BarChart, Block, Borders},
    Terminal,
};

struct App<'a> {
    data: Vec<(&'a str, u64)>,
}

impl<'a> App<'a> {
    fn new() -> App<'a> {
        App {
            data: vec![
                ("B1", 9),
                ("B2", 12),
                ("B3", 5),
                ("B4", 8),
                ("B5", 2),
                ("B6", 4),
                ("B7", 5),
                ("B8", 9),
                ("B9", 14),
                ("B10", 15),
                ("B11", 1),
                ("B12", 0),
                ("B13", 4),
                ("B14", 6),
                ("B15", 4),
                ("B16", 6),
                ("B17", 4),
                ("B18", 7),
                ("B19", 13),
                ("B20", 8),
                ("B21", 11),
                ("B22", 9),
                ("B23", 3),
                ("B24", 5),
            ],
        }
    }

    fn update(&mut self) {
        let value = self.data.pop().unwrap();
        self.data.insert(0, value);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Terminal initialization
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Setup event handlers
    let events = Events::new();

    // App
    let mut app = App::new();

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(f.size());
            let barchart = BarChart::default()
                .block(Block::default().title("Data1").borders(Borders::ALL))
                .data(&app.data)
                .bar_width(9)
                .bar_style(Style::default().fg(Color::Yellow))
                .value_style(Style::default().fg(Color::Black).bg(Color::Yellow));
            f.render_widget(barchart, chunks[0]);

            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(chunks[1]);

            let barchart = BarChart::default()
                .block(Block::default().title("Data2").borders(Borders::ALL))
                .data(&app.data)
                .bar_width(5)
                .bar_gap(3)
                .bar_style(Style::default().fg(Color::Green))
                .value_style(
                    Style::default()
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(barchart, chunks[0]);

            let barchart = BarChart::default()
                .block(Block::default().title("Data3").borders(Borders::ALL))
                .data(&app.data)
                .bar_style(Style::default().fg(Color::Red))
                .bar_width(7)
                .bar_gap(0)
                .value_style(Style::default().bg(Color::Red))
                .label_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::ITALIC),
                );
            f.render_widget(barchart, chunks[1]);
        })?;

        match events.next()? {
            Event::Input(input) => {
                if input == Key::Char('q') {
                    break;
                }
            }
            Event::Tick => {
                app.update();
            }
        }
    }

    Ok(())
}
