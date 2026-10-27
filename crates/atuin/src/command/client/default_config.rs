use std::io::{self, Write as _};

use atuin_client::settings::Settings;

pub fn run() -> io::Result<()> {
    writeln!(io::stdout(), "{}", Settings::example_config())
}
