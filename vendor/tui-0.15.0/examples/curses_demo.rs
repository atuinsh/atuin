mod demo;
#[allow(dead_code)]
mod util;

use crate::demo::{ui, App};
use argh::FromArgs;
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};
use tui::{backend::CursesBackend, Terminal};

/// Curses demo
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

    let mut backend =
        CursesBackend::new().ok_or_else(|| io::Error::new(io::ErrorKind::Other, ""))?;
    let curses = backend.get_curses_mut();
    curses.set_echo(false);
    curses.set_input_timeout(easycurses::TimeoutMode::WaitUpTo(50));
    curses.set_input_mode(easycurses::InputMode::RawCharacter);
    curses.set_keypad_enabled(true);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut app = App::new("Curses demo", cli.enhanced_graphics);

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(cli.tick_rate);
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if let Some(input) = terminal.backend_mut().get_curses_mut().get_input() {
            match input {
                easycurses::Input::Character(c) => {
                    app.on_key(c);
                }
                easycurses::Input::KeyUp => {
                    app.on_up();
                }
                easycurses::Input::KeyDown => {
                    app.on_down();
                }
                easycurses::Input::KeyLeft => {
                    app.on_left();
                }
                easycurses::Input::KeyRight => {
                    app.on_right();
                }
                _ => {}
            };
        };
        terminal.backend_mut().get_curses_mut().flush_input();
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
