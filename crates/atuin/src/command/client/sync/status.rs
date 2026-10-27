use std::fmt;
use std::io::{self, Write as _};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::api_client;
use atuin_client::settings::Settings;
use atuin_common::time::DurationExt;
use colored::Colorize;
use eyre::{Result, bail};
use time::OffsetDateTime;
use url::Url;

use crate::i18n::fl;
use crate::{SHA, VERSION};

pub async fn run(settings: &Arc<Settings>) -> Result<()> {
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
    let last_sync = settings.last_sync().await?;

    let status = SyncStatus {
        auto_sync: settings.auto_sync.then(|| AutoSync {
            frequency: settings.sync_frequency,
            last_sync: last_sync.to_offset(settings.timezone.0),
        }),
        // Shown only with auto sync, as it has been since `atuin status` began (#830).
        remote: settings.auto_sync.then(|| Remote {
            address: settings.sync_address.clone(),
            username: me.username,
        }),
    };
    writeln!(io::stdout(), "{status}")?;

    Ok(())
}

/// What `atuin sync status` shows.
struct SyncStatus {
    /// How often this machine syncs on its own, and when it last did; `None` without auto sync.
    auto_sync: Option<AutoSync>,
    remote: Option<Remote>,
}

struct AutoSync {
    frequency: Duration,
    last_sync: OffsetDateTime,
}

/// The sync server, and who this machine is logged in as.
struct Remote {
    address: Url,
    username: String,
}

impl fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}\n", fl!("sync-status-version", version = VERSION, sha = SHA))?;
        write!(f, "{}", fl!("sync-status-local").green())?;

        if let Some(auto_sync) = &self.auto_sync {
            let frequency = auto_sync.frequency.display().largest_unit().to_string();
            write!(f, "\n{}", fl!("sync-status-frequency", frequency = frequency))?;
            let time = auto_sync.last_sync.to_string();
            write!(f, "\n{}", fl!("sync-status-last-sync", time = time))?;
        }

        if let Some(remote) = &self.remote {
            write!(f, "\n{}", fl!("sync-status-remote").green())?;
            let address = remote.address.to_string();
            write!(f, "\n{}", fl!("sync-status-address", address = address))?;
            write!(f, "\n{}", fl!("sync-status-username", username = remote.username.clone()))?;
        }

        Ok(())
    }
}
