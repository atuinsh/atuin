use atuin_client::settings::Settings;
use atuin_common::userio;
use eyre::{Result, bail};

use crate::i18n::fl;

pub async fn run(settings: &Settings) -> Result<()> {
    let meta = settings.meta_store().await?;

    let cli_token = meta.session_token().await?;
    let hub_token = meta.hub_session_token().await?;

    let Some(cli_token) = cli_token else {
        bail!(fl!("link-no-cli-session"));
    };

    let hub_address = settings.hub_endpoint();

    if hub_token.is_some() {
        userio::log::info!("link-both-sessions");
    } else {
        userio::log::info!("link-hub-login-first");

        let session = atuin_client::hub::HubAuthSession::start(&hub_address).await?;

        userio::log::info!(
            "link-hub-open",
            instructions = if session.open_in_browser() {
                fl!("account-hub-browser-opened")
            } else {
                fl!("account-hub-open-url")
            },
            url = session.auth_url.to_string()
        );

        let token = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
                &fl!("account-hub-waiting"),
            )
            .await?;

        atuin_client::hub::save_session(&token, settings).await?;
        userio::log::info!("link-hub-complete");
    }

    atuin_client::hub::link_account(&hub_address, &cli_token, settings).await?;
    userio::log::info!("link-success");

    Ok(())
}
