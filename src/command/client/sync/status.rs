use atuin_client::{
    api_client, database::Database, encryption::load_encoded_key, settings::Settings,
};
use colored::*;
use eyre::Result;

pub async fn run(settings: &Settings, db: &impl Database) -> Result<()> {
    let client = api_client::Client::new(
        &settings.sync_address,
        &settings.session_token,
        load_encoded_key(settings)?,
    )?;

    let status = client.status().await?;
    let last_sync = Settings::last_sync()?;
    let local_count = db.history_count().await?;

    println!("{}", "[Local]".green());

    if settings.auto_sync {
        println!("Sync frequency: {}", settings.sync_frequency);
        println!("Last sync: {}", last_sync);
    }

    println!("History count: {}\n", local_count);

    if settings.auto_sync {
        println!("{}", "[Remote]".green());
        println!("Address: {}", settings.sync_address);
        println!("Username: {}", status.username);
        println!("History count: {}", status.count);
    }

    Ok(())
}
