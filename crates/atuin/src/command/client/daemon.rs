use eyre::Result;

use atuin_client::settings::Settings;
use atuin_daemon::server::listen;

pub async fn run(settings: &Settings) -> Result<()> {
    listen("/Users/ellie/.atuin.sock".into()).await?;

    Ok(())
}
