use std::path::PathBuf;

use crate::{SHA, VERSION};
use atuin_client::{api_client, database::Database, settings::Settings};
use colored::Colorize;
use eyre::Result;

pub async fn run(settings: &Settings, db: &impl Database) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if !PathBuf::from(session_path).exists() {
        println!("You are not logged in to a sync server - cannot show sync status");

        return Ok(());
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token()?.as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    let status = client.status().await?;
    let last_sync = Settings::last_sync()?;

    println!("Atuin v{VERSION} - Build rev {SHA}\n");

    println!("{}", "[Local]".green());

    if settings.auto_sync {
        println!("Sync frequency: {}", settings.sync_frequency);
        println!("Last sync: {last_sync}");
    }

    if !settings.sync.records {
        let local_count = db.history_count(false).await?;
        let deleted_count = db.history_count(true).await? - local_count;

        println!("History count: {local_count}");
        println!("Deleted history count: {deleted_count}\n");
    }

    if settings.auto_sync {
        println!("{}", "[Remote]".green());
        println!("Address: {}", settings.sync_address);
        println!("Username: {}", status.username);
    }

    Ok(())
}
