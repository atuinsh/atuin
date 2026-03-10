use eyre::{Result, bail};

use atuin_client::settings::Settings;

use super::DEFAULT_HUB_ENDPOINT;

pub async fn run(settings: &Settings) -> Result<()> {
    let meta = Settings::meta_store().await?;

    let cli_token = meta.session_token().await?;
    let hub_token = meta.hub_session_token().await?;

    let Some(cli_token) = cli_token else {
        bail!("No CLI session found. Please log in first with 'atuin login'.");
    };

    let hub_address = settings
        .active_hub_endpoint()
        .unwrap_or_else(|| DEFAULT_HUB_ENDPOINT.to_string());

    if hub_token.is_some() {
        println!("Found both Hub and CLI sessions. Linking accounts...");
    } else {
        println!("Found CLI session but no Hub session. Logging in to Hub first...");

        let session = atuin_client::hub::HubAuthSession::start(&hub_address).await?;
        println!("Open this URL to authenticate with Atuin Hub:");
        println!("{}", session.auth_url);

        let token = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
            )
            .await?;

        atuin_client::hub::save_session(&token).await?;
        println!("Hub authentication complete.");
    }

    atuin_client::hub::link_account(&hub_address, &cli_token).await?;
    println!("Successfully linked CLI account to Hub.");

    Ok(())
}
