extern crate colored;

use colored::*;

/*
 * This example use colored strings in a nested way (at line 14). It shows that colored is able to
 * keep the correct color on the “!lalalala” part.
 */

fn main() {
    let world = "world".bold();
    let hello_world = format!("Hello, {}!", world);
    println!("{}", hello_world);
    let hello_world = format!("Hello, {}!lalalala", world).red();
    println!("{}", hello_world);
}
