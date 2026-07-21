use crate::{SHA, VERSION};
use atuin_client::{api_client, settings::Settings};
use colored::Colorize;
use eyre::{Result, bail};

pub async fn run(settings: &Settings) -> Result<()> {
    if !settings.logged_in().await? {
        bail!("You are not logged in to a sync server - cannot show sync status");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.sync_auth_token().await?,
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let me = client.me().await?;
    let last_sync = Settings::last_sync().await?;

    println!("Atuin v{VERSION} - Build rev {SHA}\n");

    println!("{}", "[Local]".green());

    if settings.auto_sync {
        println!("Sync frequency: {}", settings.sync_frequency);
        println!("Last sync: {}", last_sync.to_offset(settings.timezone.0));
    }

    if settings.auto_sync {
        println!("{}", "[Remote]".green());
        println!("Address: {}", settings.sync_address);
        println!("Username: {}", me.username);
    }

    Ok(())
}
