extern crate termion;

use termion::color::{DetectColors, AnsiValue, Bg};
use termion::raw::IntoRawMode;
use std::io::stdout;

fn main() {
    let count;
    {
        let mut term = stdout().into_raw_mode().unwrap();
        count = term.available_colors().unwrap();
    }

    println!("This terminal supports {} colors.", count);
    for i in 0..count {
        print!("{} {}", Bg(AnsiValue(i as u8)), Bg(AnsiValue(0)));
    }
    println!();
}
