use atuin_common::out::outln;

static CONTRIBUTORS: &str = include_str!("CONTRIBUTORS");

pub fn run() {
    outln!("\n{CONTRIBUTORS}");
}
