extern crate termion;

use termion::event::{Key, Event, MouseEvent};
use termion::input::{TermRead, MouseTerminal};
use termion::raw::IntoRawMode;
use std::io::{Write, stdout, stdin};

fn main() {
    let stdin = stdin();
    let mut stdout = MouseTerminal::from(stdout().into_raw_mode().unwrap());

    write!(stdout,
           "{}{}q to exit. Click, click, click!",
           termion::clear::All,
           termion::cursor::Goto(1, 1))
            .unwrap();
    stdout.flush().unwrap();

    for c in stdin.events() {
        let evt = c.unwrap();
        match evt {
            Event::Key(Key::Char('q')) => break,
            Event::Mouse(me) => {
                match me {
                    MouseEvent::Press(_, x, y) => {
                        write!(stdout, "{}x", termion::cursor::Goto(x, y)).unwrap();
                    }
                    _ => (),
                }
            }
            _ => {}
        }
        stdout.flush().unwrap();
    }
}
