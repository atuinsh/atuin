use atuin_client::{api_client, encryption::load_encoded_key, settings::Settings};
use eyre::{bail, Result};
use std::path::PathBuf;

pub async fn run(settings: &Settings) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if !PathBuf::from(session_path).exists() {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        &settings.session_token,
        load_encoded_key(settings)?,
    )?;

    client.delete().await?;

    println!("Your account is deleted");

    Ok(())
}
