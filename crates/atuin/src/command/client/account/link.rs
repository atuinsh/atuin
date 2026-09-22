use atuin_client::settings::Settings;
use eyre::{Result, bail};

use crate::i18n::fl;

pub async fn run(settings: &Settings) -> Result<()> {
    let meta = Settings::meta_store().await?;

    let cli_token = meta.session_token().await?;
    let hub_token = meta.hub_session_token().await?;

    let Some(cli_token) = cli_token else {
        bail!(fl!("link-no-cli-session"));
    };

    let hub_address = settings.hub_endpoint();

    if hub_token.is_some() {
        println!("{}", fl!("link-both-sessions"));
    } else {
        println!("{}", fl!("link-hub-login-first"));

        let session = atuin_client::hub::HubAuthSession::start(&hub_address).await?;
        println!("{}", fl!("account-hub-open-url"));
        println!("{}", session.auth_url);

        let token = session
            .wait_for_completion(
                atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
                atuin_client::hub::DEFAULT_POLL_INTERVAL,
            )
            .await?;

        atuin_client::hub::save_session(&token).await?;
        println!("{}", fl!("link-hub-complete"));
    }

    atuin_client::hub::link_account(&hub_address, &cli_token).await?;
    println!("{}", fl!("link-success"));

    Ok(())
}
