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
use std::time::Duration;

use atuin_api_client::{ApiBody, ApiError, MapApiError, Secret, types};
use atuin_common::futures::Backoff;
use atuin_common::url::UrlAppendExt;
use eyre::{Context, Result};
use reqwest::{StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use crate::http::hub_client;
use crate::settings::Settings;

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
    /// The hub, called anonymously.
    #[debug(skip)]
    api: atuin_api_client::Client,
}

/// The result of polling for hub auth completion
#[derive(Debug, Clone)]
pub enum HubAuthStatus {
    /// Still waiting for user authorization
    Pending,
    /// Authorization complete, contains the session token
    Complete(SecretString),
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
    /// The hub gave no answer, or one that is not the API's.
    #[error(transparent)]
    Request(ApiError),
}

impl From<ApiError> for HubError {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Status { status, reason, .. } => Self::Status { status, reason },
            ApiError::Transport(_) | ApiError::Decode(_) | ApiError::NotSent(_) => {
                Self::Request(err)
            }
        }
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

        let api = hub_client(hub_address, None)?;
        let code_response = request_code(&api).await?;

        debug!("Received code from Hub");

        let code = SecretString::from(code_response.code);
        let mut auth_url = hub_address.append_path("auth/cli")?;
        auth_url.query_pairs_mut().append_pair("code", code.expose_secret());

        Ok(Self {
            code,
            auth_url,
            hub_address: hub_address.clone(),
            api,
        })
    }

    /// Poll for the authentication status
    ///
    /// Returns the current status of the authentication flow.
    pub async fn poll(&self) -> Result<HubAuthStatus> {
        match verify_code(&self.api, &self.code).await {
            Ok(response) => {
                debug!("Authentication complete, received token");
                Ok(HubAuthStatus::Complete(response.token.into()))
            }
            // The Hub answers 401 until the user authorizes in the browser.
            Err(HubError::Status { status, .. }) if status == StatusCode::UNAUTHORIZED => {
                Ok(HubAuthStatus::Pending)
            }
            // The hub spends the code on its 200, so a 200 without a token can never complete.
            Err(err @ HubError::Request(ApiError::Decode(_))) => {
                Err(eyre::Report::new(err).wrap_err("Authentication failed"))
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
    ) -> Result<SecretString> {
        debug!("Polling for Hub authentication completion...");

        Backoff::Constant(poll_interval)
            .retry(
                || async move {
                    match self.poll().await {
                        Ok(HubAuthStatus::Complete(token)) => ControlFlow::Break(Ok(token)),
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
            })
    }
}

/// Save a hub session token
///
/// This saves the token to the meta store so it can be used for subsequent Hub API calls.
/// Note: This is separate from the sync session token.
pub async fn save_session(token: &SecretString) -> Result<()> {
    Settings::meta_store()
        .await?
        .save_hub_session(token)
        .await
        .context("Failed to save hub session")
}

/// Delete the hub session token (logout from Hub)
pub async fn delete_session() -> Result<()> {
    Settings::meta_store().await?.delete_hub_session().await.context("Failed to delete hub session")
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
pub async fn get_session_token() -> Result<Option<SecretString>> {
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
/// - The Hub account is already linked to a different CLI account
///
/// A CLI account that is already linked, to this Hub account or another, is not an error.
pub async fn link_account(hub_address: &Url, cli_token: &SecretString) -> Result<()> {
    let hub_token = get_session_token()
        .await?
        .ok_or_else(|| eyre::eyre!("Not logged in to Hub - cannot link account"))?;

    debug!("Linking CLI account to Hub at {}", hub_address);

    link(&hub_client(hub_address, Some(&hub_token))?, cli_token).await?;

    info!("Successfully linked CLI account to Hub");
    Ok(())
}

/// Link the CLI account whose session is `cli_token` to the hub account `api` authenticates as.
async fn link(api: &atuin_api_client::Client, cli_token: &SecretString) -> Result<(), HubError> {
    let body = types::LinkAccountRequest {
        token: cli_token.clone().into(),
    };

    match api.link_account(&body).map_api_error().await {
        Ok(_) => Ok(()),
        // The CLI account is already linked, possibly to a different hub account.
        Err(ApiError::Status {
            status: StatusCode::CONFLICT,
            ..
        }) => {
            debug!("CLI account already linked to a Hub account");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Request a CLI auth code from the Atuin Hub
async fn request_code(api: &atuin_api_client::Client) -> Result<types::CliCodeResponse, HubError> {
    debug!("Requesting code from Hub");

    Ok(api.create_cli_auth_code().body().await?)
}

/// Poll to verify the CLI auth code and get the session token
async fn verify_code(
    api: &atuin_api_client::Client,
    code: &SecretString,
) -> Result<types::CliVerifyResponse, HubError> {
    debug!("Verifying code with Hub");

    let code = Secret::from(code.clone());
    Ok(api.verify_cli_auth_code(&code).body().await?)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const CODE: &str = "s3cret-code";

    #[rstest]
    fn debug_omits_the_auth_code() {
        let hub = Url::parse("https://hub.example").unwrap();
        let mut auth_url = hub.clone();
        auth_url.query_pairs_mut().append_pair("code", CODE);
        let session = HubAuthSession {
            code: SecretString::from(CODE),
            auth_url,
            api: hub_client(&hub, None).unwrap(),
            hub_address: hub,
        };

        assert!(!format!("{session:?}").contains(CODE));
    }
}
