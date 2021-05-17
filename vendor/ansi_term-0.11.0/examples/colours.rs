extern crate ansi_term;
use ansi_term::Colour::*;

fn main() {
    println!("{}", Black.paint("Black"));
    println!("{}", Red.paint("Red"));
    println!("{}", Green.paint("Green"));
    println!("{}", Yellow.paint("Yellow"));
    println!("{}", Blue.paint("Blue"));
    println!("{}", Purple.paint("Purple"));
    println!("{}", Cyan.paint("Cyan"));
    println!("{}", White.paint("White"));
}
