extern crate colored;

use colored::*;

fn main() {
    // TADAA !
    println!(
        "{} {} {}!",
        "it".green(),
        "works".blue().bold(),
        "great".bold().yellow()
    );

    println!("{}", String::from("this also works!").green().bold());

    let mut s = String::new();
    s.push_str(&"why not ".red().to_string());
    s.push_str(&"push things ".blue().to_string());
    s.push_str(&"a little further ?".green().to_string());
    println!("{}", s);

    let s = format!("{} {} {}", "this".red(), "is".blue(), "easier".green());
    println!("{}", s);
}
