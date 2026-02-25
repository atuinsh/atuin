//! Hub authentication support for Atuin
//!
//! This module provides programmatic access to the Atuin Hub authentication flow.
//! It can be used by other crates (like atuin-ai) to authenticate with the Hub
//! and obtain session tokens.
//!
//! Hub authentication is separate from sync authentication - users can have both
//! a sync session (for history sync) and a hub session (for Hub-specific features
//! like AI).

use std::time::Duration;

use eyre::{Context, Result, bail};
use reqwest::{StatusCode, Url, header::USER_AGENT};

use atuin_common::{
    api::{
        ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, CliCodeResponse, CliVerifyResponse,
        ErrorResponse,
    },
    tls::ensure_crypto_provider,
};

use crate::settings::Settings;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

/// The result of starting a hub authentication flow
#[derive(Debug, Clone)]
pub struct HubAuthSession {
    /// The code to be verified
    pub code: String,
    /// The URL the user should visit to authenticate
    pub auth_url: String,
    /// The hub address being used
    pub hub_address: String,
}

/// The result of polling for hub auth completion
#[derive(Debug, Clone)]
pub enum HubAuthStatus {
    /// Still waiting for user authorization
    Pending,
    /// Authorization complete, contains the session token
    Complete(String),
    /// Authorization failed with an error
    Failed(String),
}

/// Default poll interval for checking auth status
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default timeout for the entire auth flow
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(600);

impl HubAuthSession {
    /// Start a new hub authentication session
    ///
    /// Returns a session containing the code and auth URL that the user should visit.
    pub async fn start(settings: &Settings) -> Result<Self> {
        debug!("Starting Hub authentication process...");

        let code_response = request_code(&settings.hub_address)
            .await
            .context("Failed to request authentication code from Hub")?;

        debug!("Received code from Hub");

        let code = code_response.code;
        let auth_url = format!("{}/auth/cli?code={}", settings.hub_address, code);

        Ok(Self {
            code,
            auth_url,
            hub_address: settings.hub_address.clone(),
        })
    }

    /// Poll for the authentication status
    ///
    /// Returns the current status of the authentication flow.
    pub async fn poll(&self) -> Result<HubAuthStatus> {
        match verify_code(&self.hub_address, &self.code).await {
            Ok(response) => {
                if let Some(token) = response.token {
                    debug!("Authentication complete, received token");
                    Ok(HubAuthStatus::Complete(token))
                } else if let Some(error) = response.error {
                    error!("Authentication failed: {}", error);
                    Ok(HubAuthStatus::Failed(error))
                } else {
                    Ok(HubAuthStatus::Pending)
                }
            }
            Err(e) => {
                // Transient errors shouldn't fail the whole flow
                log::debug!("Verification poll failed: {}", e);
                Ok(HubAuthStatus::Pending)
            }
        }
    }

    /// Poll until completion or timeout
    ///
    /// This is a convenience method that polls repeatedly until the auth completes
    /// or times out.
    pub async fn wait_for_completion(
        &self,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<String> {
        let start = std::time::Instant::now();

        debug!("Polling for Hub authentication completion...");

        loop {
            if start.elapsed() > timeout {
                warn!("Authentication loop exited due to timeout");
                bail!("Authentication timed out. Please try again.");
            }

            match self.poll().await? {
                HubAuthStatus::Complete(token) => return Ok(token),
                HubAuthStatus::Failed(error) => bail!("Authentication failed: {}", error),
                HubAuthStatus::Pending => {
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }
    }
}

/// Save a hub session token
///
/// This saves the token to the meta store so it can be used for subsequent Hub API calls.
/// Note: This is separate from the sync session token.
pub async fn save_session(token: &str) -> Result<()> {
    Settings::meta_store()
        .await?
        .save_hub_session(token)
        .await
        .context("Failed to save hub session")
}

/// Delete the hub session token (logout from Hub)
pub async fn delete_session() -> Result<()> {
    Settings::meta_store()
        .await?
        .delete_hub_session()
        .await
        .context("Failed to delete hub session")
}

/// Check if the user is logged in with Hub authentication
///
/// Returns true if the user has a valid Hub session token.
/// This is independent of whether they have a sync session.
pub async fn is_logged_in() -> Result<bool> {
    Settings::meta_store().await?.hub_logged_in().await
}

/// Get the hub session token if available
///
/// Returns the Hub session token if the user is logged in with Hub auth,
/// or None if not logged in.
pub async fn get_session_token() -> Result<Option<String>> {
    Settings::meta_store().await?.hub_session_token().await
}

// --- Internal HTTP functions ---

fn make_url(address: &str, path: &str) -> Result<String> {
    let address = if address.ends_with('/') {
        address.to_string()
    } else {
        format!("{address}/")
    };

    let path = path.strip_prefix('/').unwrap_or(path);

    let url = Url::parse(&address)
        .context("failed to parse hub address")?
        .join(path)
        .context("failed to join hub URL path")?;

    Ok(url.to_string())
}

async fn handle_resp_error(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();

    if status == StatusCode::SERVICE_UNAVAILABLE {
        error!("Service unavailable: check https://status.atuin.sh");
        bail!("Service unavailable: check https://status.atuin.sh");
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        error!("Rate limited; please wait before trying again");
        bail!("Rate limited; please wait before trying again");
    }

    if !status.is_success() {
        if let Ok(error) = resp.json::<ErrorResponse>().await {
            error!("Hub error: {} - {}", status, error.reason);
            bail!("Hub error: {} - {}", status, error.reason);
        }
        error!("Hub request failed with status: {}", status);
        bail!("Hub request failed with status: {}", status);
    }

    Ok(resp)
}

/// Request a CLI auth code from the Atuin Hub
async fn request_code(address: &str) -> Result<CliCodeResponse> {
    ensure_crypto_provider();
    let url = make_url(address, "/auth/cli/code")?;
    let client = reqwest::Client::new();

    debug!("Requesting code from Hub at {url}");

    let resp = client
        .post(&url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let code_response = resp.json::<CliCodeResponse>().await?;
    Ok(code_response)
}

/// Poll to verify the CLI auth code and get the session token
async fn verify_code(address: &str, code: &str) -> Result<CliVerifyResponse> {
    ensure_crypto_provider();
    let base = make_url(address, "/auth/cli/verify")?;
    let url = format!("{base}?code={code}");
    let client = reqwest::Client::new();

    debug!("Verifying code with Hub at {base}?code=******");

    let resp = client
        .post(&url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let verify_response = resp.json::<CliVerifyResponse>().await?;
    Ok(verify_response)
}
