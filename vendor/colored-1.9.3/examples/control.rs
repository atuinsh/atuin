extern crate colored;
use colored::*;

#[cfg(not(windows))]
fn main() {
    both()
}

#[cfg(windows)]
fn main() {
    both();

    // additional control setting using windows set_virtual_terminal
    colored::control::set_virtual_terminal(true);
    println!("{}", "stdout: Virtual Terminal is in use".bright_green());
    colored::control::set_virtual_terminal(false);
    println!(
        "{}",
        "stderr: Virtual Terminal is NOT in use, escape chars should be visible".bright_red()
    );
    colored::control::set_virtual_terminal(true);
    println!(
        "{}",
        "stdout: Virtual Terminal is in use AGAIN and should be green!".bright_green()
    );
    colored::control::set_virtual_terminal(true);

    // again with stderr
    eprintln!("{}", "stderr: Virtual Terminal is in use".bright_green());
    colored::control::set_virtual_terminal(false);
    eprintln!(
        "{}",
        "stderr: Virtual Terminal is NOT in use, escape chars should be visible".bright_red()
    );
    colored::control::set_virtual_terminal(true);
    eprintln!(
        "{}",
        "stderr: Virtual Terminal is in use AGAIN and should be green!".bright_green()
    );
}

fn both() {
    // this will be yellow if your environment allow it
    println!("{}", "some warning".yellow());
    // now , this will be always yellow
    colored::control::set_override(true);
    println!("{}", "some warning".yellow());
    // now, this will be never yellow
    colored::control::set_override(false);
    println!("{}", "some warning".yellow());
    // let the environment decide again
    colored::control::unset_override();
}
