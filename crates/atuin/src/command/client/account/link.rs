use atuin_client::settings::Settings;
use atuin_common::logs::USER;
use atuin_common::userio::outln;
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
        tracing::info!(target: USER, "{}", fl!("link-both-sessions"));
    } else {
        tracing::info!(target: USER, "{}", fl!("link-hub-login-first"));

        let session = atuin_client::hub::HubAuthSession::start(&hub_address).await?;

        if session.open_in_browser() {
            outln!("{}", fl!("account-hub-browser-opened"));
        } else {
            outln!("{}", fl!("account-hub-open-url"));
        }
        outln!("{}", session.auth_url);

        let token = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
                &fl!("account-hub-waiting"),
            )
            .await?;

        atuin_client::hub::save_session(&token, settings).await?;
        tracing::info!(target: USER, "{}", fl!("link-hub-complete"));
    }

    atuin_client::hub::link_account(&hub_address, &cli_token, settings).await?;
    tracing::info!(target: USER, "{}", fl!("link-success"));

    Ok(())
}
