//! Hub authentication support for Atuin
//!
//! This module provides programmatic access to the Atuin Hub authentication flow.
//! It can be used by other crates (like atuin-ai) to authenticate with the Hub
//! and obtain session tokens.
//!
//! Hub authentication is separate from sync authentication - users can have both
//! a sync session (for history sync) and a hub session (for Hub-specific features
//! like AI).

use std::ops::ControlFlow;
use std::process::Stdio;
use std::time::Duration;

use atuin_common::futures::Backoff;
use atuin_common::url::UrlAppendExt;
use atuin_domain::api::{
    ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, CliCodeResponse, CliVerifyResponse, ErrorResponse,
    LinkAccountRequest,
};
use eyre::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::USER_AGENT;
use reqwest::{StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use crate::settings::Settings;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

/// The result of starting a hub authentication flow
#[derive(derive_more::Debug, Clone)]
pub struct HubAuthSession {
    /// The code to be verified
    pub code: SecretString,
    /// The URL the user should visit to authenticate. Carries `code` in its query.
    #[debug(skip)]
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
    Complete(SecretString),
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
    Request(reqwest::Error),
    #[error("invalid hub URL: {0}")]
    Url(#[from] atuin_common::url::UrlAppendError),
}

impl From<reqwest::Error> for HubError {
    fn from(err: reqwest::Error) -> Self {
        // The verify endpoint carries the auth code in its query string.
        Self::Request(err.without_url())
    }
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
        auth_url.query_pairs_mut().append_pair("code", code.expose_secret());

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

    /// Best-effort attempt to open `auth_url` in the user's browser.
    ///
    /// Returns `false` when there's no browser the user could see (an SSH session, headless
    /// Linux) or the launcher failed to start, so callers should always print the URL as well.
    #[must_use]
    pub fn open_in_browser(&self) -> bool {
        let Some(program) = browser_launcher(|var| atuin_common::env::var_os(var).is_some()) else {
            return false;
        };

        // Launchers like xdg-open can be chatty and some block until the browser exits, so
        // spawn detached from our terminal rather than waiting on it.
        let child = std::process::Command::new(program)
            .arg(self.auth_url.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        match child {
            Ok(mut child) => {
                // Reap off-thread so a finished launcher doesn't linger as a zombie for the
                // whole auth poll.
                std::thread::spawn(move || child.wait());
                true
            }
            Err(_) => false,
        }
    }

    /// Poll until completion or timeout, showing `waiting_message` beside a spinner
    ///
    /// This is a convenience method that polls repeatedly until the auth completes
    /// or times out.
    pub async fn wait_for_completion(
        &self,
        timeout: Duration,
        poll_interval: Duration,
        waiting_message: &str,
    ) -> Result<SecretString> {
        debug!("Polling for Hub authentication completion...");

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(ProgressStyle::with_template("{spinner:.blue} {msg}")?);
        spinner.set_message(waiting_message.to_owned());
        spinner.enable_steady_tick(Duration::from_millis(100));

        let result = Backoff::Constant(poll_interval)
            .retry(
                || async move {
                    match self.poll().await {
                        Ok(HubAuthStatus::Complete(token)) => ControlFlow::Break(Ok(token)),
                        Ok(HubAuthStatus::Failed(error)) => {
                            ControlFlow::Break(Err(eyre::eyre!("Authentication failed: {error}")))
                        }
                        Ok(HubAuthStatus::Pending) => ControlFlow::Continue(()),
                        Err(err) => ControlFlow::Break(Err(err)),
                    }
                },
                timeout,
            )
            .await
            .unwrap_or_else(|_| {
                warn!("Authentication loop exited due to timeout");
                Err(eyre::eyre!("Authentication timed out. Please try again."))
            });

        spinner.finish_and_clear();

        result
    }
}

/// The program that opens a URL in the user's browser, or `None` when a
/// browser would open somewhere the user can't see it.
fn browser_launcher(is_set: impl Fn(&str) -> bool) -> Option<&'static str> {
    // Over SSH, a launcher would open the browser on the remote machine's desktop.
    if is_set("SSH_CONNECTION") || is_set("SSH_TTY") {
        return None;
    }

    if cfg!(target_os = "macos") {
        Some("open")
    } else if cfg!(windows) {
        // `cmd /c start` would need quoting for `&` in URLs; explorer takes the URL as-is.
        Some("explorer")
    } else if is_set("DISPLAY") || is_set("WAYLAND_DISPLAY") {
        Some("xdg-open")
    } else {
        None
    }
}

/// Save a hub session token
///
/// This saves the token to the meta store so it can be used for subsequent Hub API calls.
/// Note: This is separate from the sync session token.
pub async fn save_session(token: &SecretString, settings: &Settings) -> Result<()> {
    settings.meta_store().await?.save_hub_session(token).await.context("Failed to save hub session")
}

/// Delete the hub session token (logout from Hub)
pub async fn delete_session(settings: &Settings) -> Result<()> {
    settings.meta_store().await?.delete_hub_session().await.context("Failed to delete hub session")
}

/// Check if the user is logged in with Hub authentication
///
/// Returns true if the user has a valid Hub session token.
/// This is independent of whether they have a sync session.
pub async fn is_logged_in(settings: &Settings) -> Result<bool> {
    settings.meta_store().await?.hub_logged_in().await
}

/// Get the hub session token if available
///
/// Returns the Hub session token if the user is logged in with Hub auth,
/// or None if not logged in.
pub async fn get_session_token(settings: &Settings) -> Result<Option<SecretString>> {
    settings.meta_store().await?.hub_session_token().await
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
pub async fn link_account(
    hub_address: &Url,
    cli_token: &SecretString,
    settings: &Settings,
) -> Result<()> {
    let hub_token = get_session_token(settings)
        .await?
        .ok_or_else(|| eyre::eyre!("Not logged in to Hub - cannot link account"))?;

    let url = hub_address.append_path("api/v0/account/link")?;

    debug!("Linking CLI account to Hub at {}", hub_address);

    let client = reqwest::Client::new();

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .bearer_auth(hub_token.expose_secret())
        .json(&LinkAccountRequest {
            token: cli_token.clone(),
        })
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

    let reason = resp.json::<ErrorResponse>().await.ok().map(|e| e.reason.into_owned());
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
async fn verify_code(address: &Url, code: &SecretString) -> Result<CliVerifyResponse, HubError> {
    let mut url = address.append_path("auth/cli/verify")?;
    let client = reqwest::Client::new();

    // Logged before the code is appended, so the secret stays out of the logs.
    debug!("Verifying code with Hub at {url}");

    url.query_pairs_mut().append_pair("code", code.expose_secret());

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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn debug_omits_the_auth_code() {
        let hub = Url::parse("https://hub.example").unwrap();
        let mut auth_url = hub.clone();
        auth_url.query_pairs_mut().append_pair("code", "s3cret-code");
        let session = HubAuthSession {
            code: SecretString::from("s3cret-code"),
            auth_url,
            hub_address: hub,
        };

        assert!(!format!("{session:?}").contains("s3cret-code"));
    }

    #[rstest]
    #[case::ssh_connection(&["SSH_CONNECTION", "DISPLAY"])]
    #[case::ssh_tty(&["SSH_TTY", "WAYLAND_DISPLAY"])]
    fn no_browser_over_ssh(#[case] set: &[&str]) {
        assert_eq!(browser_launcher(|var| set.contains(&var)), None);
    }

    #[rstest]
    #[case::x11("DISPLAY")]
    #[case::wayland("WAYLAND_DISPLAY")]
    fn browser_with_a_local_display(#[case] display: &str) {
        assert!(browser_launcher(|var| var == display).is_some());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[rstest]
    fn no_browser_on_headless_unix() {
        assert_eq!(browser_launcher(|_| false), None);
    }
}
