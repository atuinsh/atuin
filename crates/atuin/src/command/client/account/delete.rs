use atuin_client::{api_client, settings::Settings};
use eyre::{bail, Result};
use std::fs::remove_file;
use std::path::PathBuf;

pub async fn run(settings: &Settings) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if !PathBuf::from(session_path).exists() {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token()?.as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    client.delete().await?;

    // Fixes stale session+key when account is deleted via CLI.
    if PathBuf::from(session_path).exists() {
        remove_file(PathBuf::from(session_path))?;
    }

    println!("Your account is deleted");

    Ok(())
}
