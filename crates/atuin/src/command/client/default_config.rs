use atuin_client::settings::Settings;

#[expect(clippy::disallowed_macros, reason = "prints the default config")]
pub fn run() {
    println!("{}", Settings::example_config());
}
