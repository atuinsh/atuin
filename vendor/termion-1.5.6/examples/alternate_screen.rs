extern crate termion;

use termion::screen::*;
use std::io::{Write, stdout};
use std::{time, thread};

fn main() {
    {
        let mut screen = AlternateScreen::from(stdout());
        write!(screen, "Welcome to the alternate screen.\n\nPlease wait patiently until we arrive back at the main screen in a about three seconds.").unwrap();
        screen.flush().unwrap();

        thread::sleep(time::Duration::from_secs(3));
    }

    println!("Phew! We are back.");
}
