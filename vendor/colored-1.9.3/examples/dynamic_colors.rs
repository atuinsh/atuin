extern crate colored;
use colored::*;

fn main() {
    // the easy way
    "blue string yo".color("blue");

    // this will default to white
    "white string".color("zorglub");

    // the safer way via a Result
    let color_res = "zorglub".parse(); // <- this returns a Result<Color, ()>
    "red string".color(color_res.unwrap_or(Color::Red));
}
