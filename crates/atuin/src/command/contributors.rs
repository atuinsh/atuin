use atuin_common::userio::outln;

static CONTRIBUTORS: &str = include_str!("CONTRIBUTORS");

pub fn run() {
    outln!("\n{CONTRIBUTORS}");
}
