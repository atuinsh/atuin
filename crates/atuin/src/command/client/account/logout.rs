use atuin_client::settings::Settings;
use eyre::Result;

pub fn run(settings: &Settings) -> Result<()> {
    atuin_client::logout::logout(settings)
}
