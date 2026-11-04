use atuin_client::settings::Settings;
use atuin_common::userio;
use eyre::{Result, bail};

use crate::i18n::fl;

pub async fn run(settings: &Settings) -> Result<()> {
    // Normalize any mis-stored tokens before deciding there is nothing to
    // link. A legacy CLI token left in the hub_session slot (from an earlier
    // token-storage bug) is moved to the session slot by resolve_sync_auth().
    let _ = settings.resolve_sync_auth().await;

    let meta = settings.meta_store().await?;

    let cli_token = meta.session_token().await?;
    let hub_token = meta.hub_session_token().await?;

    let Some(cli_token) = cli_token else {
        // No CLI (sync) session to link. If the user is already authenticated
        // with Hub — e.g. they logged in through the browser flow with
        // `atuin login` — their Hub identity is already their account, so
        // there is no separate CLI session to associate.
        if hub_token.is_some() {
            userio::log::info!("link-hub-authenticated");
            return Ok(());
        }

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
            url = session.auth_url.to_string(),
            code = session.short_code()
        );

        let new_session = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
                &fl!("account-hub-waiting"),
            )
            .await?;

        atuin_client::hub::save_session(&new_session, settings).await?;
        userio::log::info!("link-hub-complete");
    }

    atuin_client::hub::link_account(&hub_address, &cli_token, settings).await?;
    userio::log::info!("link-success");

    Ok(())
}
