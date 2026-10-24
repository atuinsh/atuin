static CONTRIBUTORS: &str = include_str!("CONTRIBUTORS");

#[expect(clippy::disallowed_macros, reason = "prints the contributors")]
pub fn run() {
    println!("\n{CONTRIBUTORS}");
}
