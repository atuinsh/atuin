mod demo;
#[allow(dead_code)]
mod util;

use crate::demo::{ui, App};
use argh::FromArgs;
use rustbox::keyboard::Key;
use std::{
    error::Error,
    time::{Duration, Instant},
};
use tui::{backend::RustboxBackend, Terminal};

/// Rustbox demo
#[derive(Debug, FromArgs)]
struct Cli {
    /// time in ms between two ticks.
    #[argh(option, default = "250")]
    tick_rate: u64,
    /// whether unicode symbols are used to improve the overall look of the app
    #[argh(option, default = "true")]
    enhanced_graphics: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli: Cli = argh::from_env();

    let backend = RustboxBackend::new()?;
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new("Rustbox demo", cli.enhanced_graphics);

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(cli.tick_rate);
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if let Ok(rustbox::Event::KeyEvent(key)) =
            terminal.backend().rustbox().peek_event(tick_rate, false)
        {
            match key {
                Key::Char(c) => {
                    app.on_key(c);
                }
                Key::Up => {
                    app.on_up();
                }
                Key::Down => {
                    app.on_down();
                }
                Key::Left => {
                    app.on_left();
                }
                Key::Right => {
                    app.on_right();
                }
                _ => {}
            }
        }
        if last_tick.elapsed() > tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}
