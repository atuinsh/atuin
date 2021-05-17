extern crate termion;

use termion::terminal_size;

fn main() {
    println!("Size is {:?}", terminal_size().unwrap())
}
