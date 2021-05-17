extern crate termion;

use termion::color;
use termion::raw::IntoRawMode;
use std::io::{Read, Write, stdout, stdin};

fn main() {
    // Initialize 'em all.
    let stdout = stdout();
    let mut stdout = stdout.lock().into_raw_mode().unwrap();
    let stdin = stdin();
    let stdin = stdin.lock();

    write!(stdout,
           "{}{}{}yo, 'q' will exit.{}{}",
           termion::clear::All,
           termion::cursor::Goto(5, 5),
           termion::style::Bold,
           termion::style::Reset,
           termion::cursor::Goto(20, 10))
            .unwrap();
    stdout.flush().unwrap();

    let mut bytes = stdin.bytes();
    loop {
        let b = bytes.next().unwrap().unwrap();

        match b {
                // Quit
                b'q' => return,
                // Clear the screen
                b'c' => write!(stdout, "{}", termion::clear::All),
                // Set red color
                b'r' => write!(stdout, "{}", color::Fg(color::Rgb(5, 0, 0))),
                // Write it to stdout.
                a => write!(stdout, "{}", a),
            }
            .unwrap();

        stdout.flush().unwrap();
    }
}
