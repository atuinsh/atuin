use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use clap::{Parser, Subcommand};
use eyre::Result;

/// `atuin mcp` serves over stdio, for the agents `atuin setup --mcp` connects to it.
#[derive(Parser, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    moved: Option<Moved>,
}

/// What `atuin mcp` did before setup did it, kept so that the scripts still running it say what
/// to run instead.
#[derive(Subcommand, Debug)]
enum Moved {
    #[command(hide = true)]
    Install {
        agent: Option<String>,
    },
}

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: Settings) -> Result<()> {
        if let Some(Moved::Install { agent }) = self.moved {
            let agents = agent.map_or_else(String::new, |agent| format!(" --agents {agent}"));
            return Err(super::setup::moved_here("atuin mcp install", &format!("--mcp{agents}")));
        }
        let octavo = octavo(&settings).await;
        Box::pin(atuin_ai::mcp::run(db, Arc::new(settings), octavo)).await
    }
}

/// Returns Octavo's MCP server while the user uploads threads to it. Otherwise the threads are
/// only local.
#[cfg(feature = "octavo")]
async fn octavo(settings: &Settings) -> Option<atuin_ai::mcp::Octavo> {
    use atuin_client::settings::{DEFAULT_HUB_URL, SyncAuth};

    let configs = atuin_octavo::shares::Configs::new(
        atuin_common::dirs::try_atuin_home().ok().map(std::path::PathBuf::from),
    );
    let on = configs.threads_on().await.unwrap_or_else(|err| {
        tracing::warn!(?err, "your own atuin.lua can't be run; leaving Atuin for Teams off");
        false
    });
    if !on {
        return None;
    }
    let url = if settings.hub_endpoint() == *DEFAULT_HUB_URL {
        "https://mcp.atuin.sh".to_owned()
    } else {
        settings.debug.hub_mcp_address.as_ref()?.to_string()
    };
    let SyncAuth::Hub { token } = settings.resolve_sync_auth().await else {
        return None;
    };
    let orgs = match std::env::current_dir() {
        Ok(dir) => configs.orgs_of_dir(dir).await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    Some(atuin_ai::mcp::Octavo {
        url,
        token,
        orgs: orgs.iter().map(ToString::to_string).collect(),
    })
}

/// Octavo's tools are listed into the cache this often. A session trusts the cache for longer, so
/// it does not list them itself while the daemon runs.
#[cfg(feature = "daemon")]
const KEEP_CACHED: std::time::Duration = std::time::Duration::from_mins(30);

/// Keeps Octavo's tools cached for `atuin mcp`, so that an agent session does not wait for Octavo
/// to start. The daemon runs this.
#[cfg(feature = "daemon")]
pub(super) async fn keep_octavo_tools_cached(settings: Arc<Settings>) {
    loop {
        if let Some(octavo) = octavo(&settings).await
            && let Err(e) = atuin_ai::mcp::cache_tools(&settings, octavo).await
        {
            tracing::debug!("could not cache Octavo's tools: {e:#}");
        }
        tokio::time::sleep(KEEP_CACHED).await;
    }
}

#[cfg(not(feature = "octavo"))]
#[expect(clippy::unused_async, reason = "matches the octavo build")]
async fn octavo(_: &Settings) -> Option<atuin_ai::mcp::Octavo> {
    None
}
