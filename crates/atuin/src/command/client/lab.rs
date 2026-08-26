use atuin_client::settings::Settings;
use clap::Subcommand;
use eyre::{Result, WrapErr};
use tracing::instrument;
use url::Url;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Share your terminal with others (experimental)
    Share {
        /// Allow anyone with the link to send keystrokes to your shell
        #[arg(long)]
        write: bool,

        /// Skip the confirmation prompt (the warning is still printed)
        #[arg(long)]
        yes: bool,
    },
}

impl Cmd {
    /// Async because the Hub credential accessor is async. Everything that
    /// needs `await` happens here; `run_share` receives plain data so it never
    /// has to build a nested tokio runtime.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match self {
            Self::Share { write, yes } => {
                let hub_url = lab_ws_url(settings)?;
                let api_token = lab_api_token(settings).await?;
                // `atuin_lab_share::Error` converts to `eyre::Report` via the
                // blanket `From<E: std::error::Error>`.
                Ok(atuin_lab_share::run_share(atuin_lab_share::ShareOptions {
                    yes,
                    write,
                    hub_url,
                    api_token,
                })
                .await?)
            }
        }
    }
}

/// Resolve the Hub websocket base URL from settings, honouring self-hosted Hubs
/// via `Settings::hub_endpoint()`.
///
/// `ATUIN_LAB_HUB_URL` is parsed **as given** and its scheme is never rewritten:
/// local development runs against a plain-HTTP dev hub as `ws://localhost:4000`,
/// and upgrading that to `wss` would fail the handshake. The scheme is only
/// derived (http→ws, https→wss) when the override is absent.
fn lab_ws_url(settings: &Settings) -> Result<Url> {
    if let Ok(u) = std::env::var("ATUIN_LAB_HUB_URL") {
        return Url::parse(&u).wrap_err("ATUIN_LAB_HUB_URL is not a valid URL");
    }
    let mut url = settings.hub_endpoint();
    let ws_scheme = if url.scheme() == "http" {
        "ws"
    } else {
        "wss"
    };
    let _ = url.set_scheme(ws_scheme);
    Ok(url)
}

/// The **Hub** session token — *not* the sync/`atuin-server` session token.
///
/// Do not "simplify" this to `Settings::session_token()`: they are different
/// credentials in different storage slots. Hub tokens are minted by
/// `AtuinHub.Accounts.create_api_token_for/2` with an **`atapi_` prefix** (which
/// is exactly how atuin-client's token-slot logic tells hub tokens from sync
/// tokens), and the hub authenticates this socket with
/// `Accounts.find_api_token_by(code:)` against its `api_tokens` table. A sync
/// token will never match, so using it fails 100 % of joins.
///
/// `ATUIN_LAB_HUB_TOKEN` overrides it for local development against a hub with a
/// hand-minted token (see Plan C's end-to-end task).
async fn lab_api_token(settings: &Settings) -> Result<String> {
    if let Ok(t) = std::env::var("ATUIN_LAB_HUB_TOKEN") {
        return Ok(t);
    }
    settings
        .hub_session_token()
        .await
        .wrap_err("not logged in to Atuin Hub -- run `atuin login` first")
}
