use atuin_client::{api_client, settings::Settings};
use eyre::{bail, Result};
use std::path::PathBuf;

pub async fn run(settings: &Settings) -> Result<()> {
    let session_path = settings.session_path.as_str();

    if !PathBuf::from(session_path).exists() {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(settings)?;

    client.delete().await?;

    println!("Your account is deleted");

    Ok(())
}
