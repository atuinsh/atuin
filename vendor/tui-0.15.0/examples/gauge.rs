#[allow(dead_code)]
mod util;

use crate::util::event::{Event, Events};
use std::{error::Error, io};
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge},
    Terminal,
};

struct App {
    progress1: u16,
    progress2: u16,
    progress3: f64,
    progress4: u16,
}

impl App {
    fn new() -> App {
        App {
            progress1: 0,
            progress2: 0,
            progress3: 0.0,
            progress4: 0,
        }
    }

    fn update(&mut self) {
        self.progress1 += 5;
        if self.progress1 > 100 {
            self.progress1 = 0;
        }
        self.progress2 += 10;
        if self.progress2 > 100 {
            self.progress2 = 0;
        }
        self.progress3 += 0.001;
        if self.progress3 > 1.0 {
            self.progress3 = 0.0;
        }
        self.progress4 += 3;
        if self.progress4 > 100 {
            self.progress4 = 0;
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Terminal initialization
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let events = Events::new();

    let mut app = App::new();

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                    ]
                    .as_ref(),
                )
                .split(f.size());

            let gauge = Gauge::default()
                .block(Block::default().title("Gauge1").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Yellow))
                .percent(app.progress1);
            f.render_widget(gauge, chunks[0]);

            let label = format!("{}/100", app.progress2);
            let gauge = Gauge::default()
                .block(Block::default().title("Gauge2").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Magenta).bg(Color::Green))
                .percent(app.progress2)
                .label(label);
            f.render_widget(gauge, chunks[1]);

            let gauge = Gauge::default()
                .block(Block::default().title("Gauge3").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Yellow))
                .ratio(app.progress3);
            f.render_widget(gauge, chunks[2]);

            let label = format!("{}/100", app.progress2);
            let gauge = Gauge::default()
                .block(Block::default().title("Gauge4"))
                .gauge_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::ITALIC),
                )
                .percent(app.progress4)
                .label(label)
                .use_unicode(true);
            f.render_widget(gauge, chunks[3]);
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
