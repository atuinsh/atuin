#[allow(dead_code)]
mod util;

use crate::util::event::{Config, Event, Events};
use std::{error::Error, io, time::Duration};
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    widgets::{
        canvas::{Canvas, Map, MapResolution, Rectangle},
        Block, Borders,
    },
    Terminal,
};

struct App {
    x: f64,
    y: f64,
    ball: Rectangle,
    playground: Rect,
    vx: f64,
    vy: f64,
    dir_x: bool,
    dir_y: bool,
}

impl App {
    fn new() -> App {
        App {
            x: 0.0,
            y: 0.0,
            ball: Rectangle {
                x: 10.0,
                y: 30.0,
                width: 10.0,
                height: 10.0,
                color: Color::Yellow,
            },
            playground: Rect::new(10, 10, 100, 100),
            vx: 1.0,
            vy: 1.0,
            dir_x: true,
            dir_y: true,
        }
    }

    fn update(&mut self) {
        if self.ball.x < self.playground.left() as f64
            || self.ball.x + self.ball.width > self.playground.right() as f64
        {
            self.dir_x = !self.dir_x;
        }
        if self.ball.y < self.playground.top() as f64
            || self.ball.y + self.ball.height > self.playground.bottom() as f64
        {
            self.dir_y = !self.dir_y;
        }

        if self.dir_x {
            self.ball.x += self.vx;
        } else {
            self.ball.x -= self.vx;
        }

        if self.dir_y {
            self.ball.y += self.vy;
        } else {
            self.ball.y -= self.vy
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

    // Setup event handlers
    let config = Config {
        tick_rate: Duration::from_millis(250),
        ..Default::default()
    };
    let events = Events::with_config(config);

    // App
    let mut app = App::new();

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(f.size());
            let canvas = Canvas::default()
                .block(Block::default().borders(Borders::ALL).title("World"))
                .paint(|ctx| {
                    ctx.draw(&Map {
                        color: Color::White,
                        resolution: MapResolution::High,
                    });
                    ctx.print(app.x, -app.y, "You are here", Color::Yellow);
                })
                .x_bounds([-180.0, 180.0])
                .y_bounds([-90.0, 90.0]);
            f.render_widget(canvas, chunks[0]);
            let canvas = Canvas::default()
                .block(Block::default().borders(Borders::ALL).title("Pong"))
                .paint(|ctx| {
                    ctx.draw(&app.ball);
                })
                .x_bounds([10.0, 110.0])
                .y_bounds([10.0, 110.0]);
            f.render_widget(canvas, chunks[1]);
        })?;

        match events.next()? {
            Event::Input(input) => match input {
                Key::Char('q') => {
                    break;
                }
                Key::Down => {
                    app.y += 1.0;
                }
                Key::Up => {
                    app.y -= 1.0;
                }
                Key::Right => {
                    app.x += 1.0;
                }
                Key::Left => {
                    app.x -= 1.0;
                }

                _ => {}
            },
            Event::Tick => {
                app.update();
            }
        }
    }

    Ok(())
}
