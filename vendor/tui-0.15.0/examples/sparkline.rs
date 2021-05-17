#[allow(dead_code)]
mod util;

use crate::util::{
    event::{Event, Events},
    RandomSignal,
};
use std::{error::Error, io};
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Sparkline},
    Terminal,
};

struct App {
    signal: RandomSignal,
    data1: Vec<u64>,
    data2: Vec<u64>,
    data3: Vec<u64>,
}

impl App {
    fn new() -> App {
        let mut signal = RandomSignal::new(0, 100);
        let data1 = signal.by_ref().take(200).collect::<Vec<u64>>();
        let data2 = signal.by_ref().take(200).collect::<Vec<u64>>();
        let data3 = signal.by_ref().take(200).collect::<Vec<u64>>();
        App {
            signal,
            data1,
            data2,
            data3,
        }
    }

    fn update(&mut self) {
        let value = self.signal.next().unwrap();
        self.data1.pop();
        self.data1.insert(0, value);
        let value = self.signal.next().unwrap();
        self.data2.pop();
        self.data2.insert(0, value);
        let value = self.signal.next().unwrap();
        self.data3.pop();
        self.data3.insert(0, value);
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

    // Create default app state
    let mut app = App::new();

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Length(7),
                        Constraint::Min(0),
                    ]
                    .as_ref(),
                )
                .split(f.size());
            let sparkline = Sparkline::default()
                .block(
                    Block::default()
                        .title("Data1")
                        .borders(Borders::LEFT | Borders::RIGHT),
                )
                .data(&app.data1)
                .style(Style::default().fg(Color::Yellow));
            f.render_widget(sparkline, chunks[0]);
            let sparkline = Sparkline::default()
                .block(
                    Block::default()
                        .title("Data2")
                        .borders(Borders::LEFT | Borders::RIGHT),
                )
                .data(&app.data2)
                .style(Style::default().bg(Color::Green));
            f.render_widget(sparkline, chunks[1]);
            // Multiline
            let sparkline = Sparkline::default()
                .block(
                    Block::default()
                        .title("Data3")
                        .borders(Borders::LEFT | Borders::RIGHT),
                )
                .data(&app.data3)
                .style(Style::default().fg(Color::Red));
            f.render_widget(sparkline, chunks[2]);
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
