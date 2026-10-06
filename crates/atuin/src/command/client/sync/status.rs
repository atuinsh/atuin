use atuin_client::api_client;
use atuin_client::settings::Settings;
use atuin_common::time::DurationExt;
use colored::Colorize;
use eyre::{Result, bail};

use crate::i18n::fl;
use crate::{SHA, VERSION};

pub async fn run(settings: &Settings) -> Result<()> {
    if !settings.logged_in().await? {
        bail!(fl!("sync-status-not-logged-in"));
    }

    let caps = api_client::caps_client(settings)?;
    let client = api_client::Client::new(
        settings.sync_address.clone(),
        &settings.sync_auth_token().await?,
        settings.network_connect_timeout,
        settings.network_timeout,
        &settings.extra_headers,
        caps,
    )?;

    let me = client.me().await?;
    let last_sync = Settings::last_sync().await?;

    println!("{}\n", fl!("sync-status-version", version = VERSION, sha = SHA));

    println!("{}", fl!("sync-status-local").green());

    if settings.auto_sync {
        println!(
            "{}",
            fl!(
                "sync-status-frequency",
                frequency = settings.sync_frequency.display().largest_unit().to_string()
            )
        );
        println!(
            "{}",
            fl!(
                "sync-status-last-sync",
                time = last_sync.to_offset(settings.timezone.0).to_string()
            )
        );
    }

    if settings.auto_sync {
        println!("{}", fl!("sync-status-remote").green());
        println!("{}", fl!("sync-status-address", address = settings.sync_address.to_string()));
        println!("{}", fl!("sync-status-username", username = me.username));
    }

    Ok(())
}
