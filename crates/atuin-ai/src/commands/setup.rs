use eyre::{Context as _, Result};

pub async fn run(api_endpoint: Option<String>) -> Result<()> {
    let settings = atuin_client::settings::Settings::new()?;
    let hub_address = api_endpoint
        .as_deref()
        .unwrap_or(settings.hub_address.as_str());

    if atuin_client::hub::get_session_token().await?.is_some() {
        println!("Already authenticated with Atuin Hub.");
        return Ok(());
    }

    println!("Authenticating with Atuin Hub...");

    let mut auth_settings = settings.clone();
    auth_settings.hub_address = hub_address.to_string();
    let session = atuin_client::hub::HubAuthSession::start(&auth_settings)
        .await
        .context("failed to start auth session")?;

    println!("Open this URL to continue:");
    println!("{}", session.auth_url);

    let token = session
        .wait_for_completion(
            atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
            atuin_client::hub::DEFAULT_POLL_INTERVAL,
        )
        .await?;

    atuin_client::hub::save_session(&token).await?;
    println!("Authenticated successfully!");

    Ok(())
}
