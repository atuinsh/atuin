use atuin_client::{api_client, settings::Settings};
use eyre::{Result, bail};

use crate::command::client::account::DEFAULT_HUB_ENDPOINT;

pub async fn run(settings: &Settings) -> Result<()> {
    let using_hub_sync = settings.is_hub_sync();
    let has_sync_session = settings.session_token().await.is_ok();
    let has_hub_session = settings.hub_session_token().await.is_ok();

    if using_hub_sync && has_hub_session {
        let endpoint = settings
            .active_hub_endpoint()
            .unwrap_or_else(|| DEFAULT_HUB_ENDPOINT.to_string());
        println!("You are authenticated with Atuin Hub.");
        println!("Manage your account on the site: {endpoint}/settings/account");
        return Ok(());
    }

    if !has_sync_session {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.sync_auth_token().await?,
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    client.delete().await?;

    // Clean up session from meta store
    Settings::meta_store().await?.delete_session().await?;
    Settings::meta_store().await?.delete_hub_session().await?;

    println!("Your account is deleted");

    Ok(())
}
