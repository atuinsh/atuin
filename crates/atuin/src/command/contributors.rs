use std::io::{self, Write as _, stdout};

static CONTRIBUTORS: &str = include_str!("CONTRIBUTORS");

pub fn run() -> io::Result<()> {
    writeln!(stdout(), "\n{CONTRIBUTORS}")
}
