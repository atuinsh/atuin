use atuin_client::settings::Settings;
use eyre::Result;

pub async fn run(settings: &Settings) -> Result<()> {
    atuin_client::logout::logout(settings).await
}
