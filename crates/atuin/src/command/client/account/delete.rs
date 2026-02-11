use atuin_client::{api_client, settings::Settings};
use eyre::{Result, bail};

pub async fn run(settings: &Settings) -> Result<()> {
    if !settings.logged_in().await? {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token().await?.as_str(),
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    client.delete().await?;

    // Clean up session from meta store
    Settings::meta_store().await?.delete_session().await?;

    println!("Your account is deleted");

    Ok(())
}
