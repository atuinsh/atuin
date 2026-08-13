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
use thiserror::Error;

use atuin_common::url::UrlAppendExt;
use atuin_domain::api::{
    ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, CliCodeResponse, CliVerifyResponse, ErrorResponse,
};

use crate::settings::Settings;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

/// The result of starting a hub authentication flow
#[derive(Debug, Clone)]
pub struct HubAuthSession {
    /// The code to be verified
    pub code: String,
    /// The URL the user should visit to authenticate
    pub auth_url: Url,
    /// The hub address being used
    pub hub_address: Url,
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

/// An error from a Hub HTTP request.
///
/// Requests never log; they return this and the caller decides how (or
/// whether) to surface it. The auth poll loop depends on that: the Hub
/// answers 401 until the user authorizes in the browser, and those must
/// stay silent.
#[derive(Debug, Error)]
pub enum HubError {
    #[error("{}", status_message(*status, reason.as_deref()))]
    Status {
        status: StatusCode,
        reason: Option<String>,
    },
    #[error("hub request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid hub URL: {0}")]
    Url(#[from] atuin_common::url::UrlAppendError),
}

fn status_message(status: StatusCode, reason: Option<&str>) -> impl std::fmt::Display {
    std::fmt::from_fn(move |f| match status {
        StatusCode::SERVICE_UNAVAILABLE => {
            write!(f, "Service unavailable: check https://status.atuin.sh")
        }
        StatusCode::TOO_MANY_REQUESTS => {
            write!(f, "Rate limited; please wait before trying again")
        }
        status if let Some(reason) = reason => {
            write!(f, "Hub error: {status} - {reason}")
        }
        status => {
            write!(f, "Hub request failed with status: {status}")
        }
    })
}

/// Default poll interval for checking auth status
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default timeout for the entire auth flow
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(600);

impl HubAuthSession {
    /// Start a new hub authentication session
    ///
    /// Returns a session containing the code and auth URL that the user should visit.
    pub async fn start(hub_address: &Url) -> Result<Self> {
        debug!("Starting Hub authentication process...");

        let code_response = request_code(hub_address).await?;

        debug!("Received code from Hub");

        let code = code_response.code;
        let mut auth_url = hub_address.append_path("auth/cli")?;
        auth_url.query_pairs_mut().append_pair("code", &code);

        Ok(Self {
            code,
            auth_url,
            hub_address: hub_address.clone(),
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
                    debug!("Authentication failed: {}", error);
                    Ok(HubAuthStatus::Failed(error))
                } else {
                    Ok(HubAuthStatus::Pending)
                }
            }
            // The Hub answers 401 until the user authorizes in the browser.
            Err(HubError::Status { status, .. }) if status == StatusCode::UNAUTHORIZED => {
                Ok(HubAuthStatus::Pending)
            }
            Err(e) => {
                // Tolerate transient errors (proxy blips, brief outages) rather
                // than failing the flow, but stay visible so a genuinely broken
                // Hub doesn't masquerade as an authentication timeout.
                warn!("Verification poll failed: {}", e);
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
                HubAuthStatus::Failed(error) => {
                    bail!("Authentication failed: {}", error);
                }
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

/// Link an existing CLI sync account to the current Hub user.
///
/// This associates the CLI's sync records with the Hub account, enabling
/// unified authentication. After linking:
/// - The Hub token can be used for sync operations
/// - Records are migrated to be accessible via Hub auth
///
/// Requires:
/// - A valid Hub session (user must be logged in to Hub)
/// - A valid CLI session token to link
///
/// Returns Ok(()) on success, or an error if:
/// - Not logged in to Hub
/// - CLI token is invalid
/// - CLI account is already linked to a different Hub account
pub async fn link_account(hub_address: &Url, cli_token: &str) -> Result<()> {
    let hub_token = get_session_token()
        .await?
        .ok_or_else(|| eyre::eyre!("Not logged in to Hub - cannot link account"))?;

    let url = hub_address.append_path("api/v0/account/link")?;

    debug!("Linking CLI account to Hub at {}", hub_address);

    let client = reqwest::Client::new();

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .bearer_auth(&hub_token)
        .json(&serde_json::json!({ "token": cli_token }))
        .send()
        .await?;

    let status = resp.status();

    if status == StatusCode::CONFLICT {
        // 409 means CLI account is already linked to a (possibly different) Hub account
        debug!("CLI account already linked to a Hub account");
        return Ok(());
    }

    handle_resp_error(resp).await?;

    info!("Successfully linked CLI account to Hub");
    Ok(())
}

// --- Internal HTTP functions ---

async fn handle_resp_error(resp: reqwest::Response) -> Result<reqwest::Response, HubError> {
    let status = resp.status();

    if status.is_success() {
        return Ok(resp);
    }

    let reason = resp
        .json::<ErrorResponse>()
        .await
        .ok()
        .map(|e| e.reason.into_owned());
    Err(HubError::Status { status, reason })
}

/// Request a CLI auth code from the Atuin Hub
async fn request_code(address: &Url) -> Result<CliCodeResponse, HubError> {
    let url = address.append_path("auth/cli/code")?;
    let client = reqwest::Client::new();

    debug!("Requesting code from Hub at {url}");

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let code_response = resp.json::<CliCodeResponse>().await?;
    Ok(code_response)
}

/// Poll to verify the CLI auth code and get the session token
async fn verify_code(address: &Url, code: &str) -> Result<CliVerifyResponse, HubError> {
    let mut url = address.append_path("auth/cli/verify")?;
    let client = reqwest::Client::new();

    // Logged before the code is appended, so the secret stays out of the logs.
    debug!("Verifying code with Hub at {url}");

    url.query_pairs_mut().append_pair("code", code);

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let verify_response = resp.json::<CliVerifyResponse>().await?;
    Ok(verify_response)
}
