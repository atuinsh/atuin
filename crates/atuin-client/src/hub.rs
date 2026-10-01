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

use atuin_api_client::{ApiError, MapApiError, Secret, types};
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

    Ok(api.create_cli_auth_code().map_api_error().await?.into_inner())
}

/// Poll to verify the CLI auth code and get the session token
async fn verify_code(
    api: &atuin_api_client::Client,
    code: &SecretString,
) -> Result<types::CliVerifyResponse, HubError> {
    debug!("Verifying code with Hub");

    let code = Secret::from(code.clone());
    Ok(api.verify_cli_auth_code(&code).map_api_error().await?.into_inner())
}

#[cfg(test)]
mod tests {
    use atuin_common::test_utils::capture_logs;
    use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_USER_AGENT};
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use serde_json::{Value, json};
    use tracing::Level;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const CODE: &str = "s3cret-code";

    /// A hub that hands out [`CODE`] for a browser login.
    #[fixture]
    async fn hub() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/cli/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": CODE})))
            .mount(&server)
            .await;
        server
    }

    /// Answer the verification of [`CODE`] with `status` and `body` the first `times` polls, if
    /// given, else on every poll.
    async fn answer_verify(hub: &MockServer, status: u16, body: Value, times: Option<u64>) {
        let mock = Mock::given(method("POST"))
            .and(path("/auth/cli/verify"))
            .and(query_param("code", CODE))
            .respond_with(ResponseTemplate::new(status).set_body_json(body));
        let mock = match times {
            Some(times) => mock.up_to_n_times(times).with_priority(1),
            None => mock,
        };
        mock.mount(hub).await;
    }

    async fn start(hub: &MockServer) -> HubAuthSession {
        HubAuthSession::start(&hub.uri().parse().unwrap()).await.unwrap()
    }

    /// What a poll came to, e.g. `complete atapi_tok`, `pending` or `error: ...`.
    fn polled(result: Result<HubAuthStatus>) -> String {
        match result {
            Ok(HubAuthStatus::Complete(token)) => format!("complete {}", token.expose_secret()),
            Ok(HubAuthStatus::Pending) => "pending".to_owned(),
            Err(err) => format!("error: {err}"),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn debug_omits_the_auth_code(#[future(awt)] hub: MockServer) {
        let session = start(&hub).await;

        assert!(session.auth_url.as_str().ends_with("/auth/cli?code=s3cret-code"));
        assert!(!format!("{session:?}").contains(CODE));
    }

    /// The hub answers 401 on every poll until the user authorizes, so only other failures warn.
    #[rstest]
    #[case::authorized(
        200,
        json!({"success": true, "token": "atapi_tok"}),
        "complete atapi_tok",
        0
    )]
    #[case::not_yet_authorized(
        401,
        json!({"error": "Not attached to a token", "reason": "Not attached to a token"}),
        "pending",
        0
    )]
    #[case::expired(404, json!({"error": "Not found", "reason": "Not found"}), "pending", 1)]
    #[case::outage(503, json!({}), "pending", 1)]
    #[case::spent_without_a_token(200, json!({"success": false}), "error: Authentication failed", 0)]
    #[tokio::test]
    async fn poll_completes_only_with_a_token(
        #[future(awt)] hub: MockServer,
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
        #[case] warnings: usize,
    ) {
        answer_verify(&hub, status, body, None).await;
        let session = start(&hub).await;
        let logs = capture_logs();

        let result = polled(session.poll().await);

        let warned = logs.get().iter().filter(|log| log.level == Level::WARN).count();
        assert_eq!((result.as_str(), warned), (expected, warnings));
    }

    #[rstest]
    #[tokio::test]
    async fn wait_for_completion_polls_until_authorized(#[future(awt)] hub: MockServer) {
        answer_verify(&hub, 401, json!({"reason": "Not attached to a token"}), Some(2)).await;
        answer_verify(&hub, 200, json!({"success": true, "token": "atapi_tok"}), None).await;

        let token = start(&hub)
            .await
            .wait_for_completion(Duration::from_secs(10), Duration::from_millis(10))
            .await
            .unwrap();

        assert_eq!(token.expose_secret(), "atapi_tok");
        let verifies = hub.received_requests().await.unwrap().into_iter();
        assert_eq!(verifies.filter(|r| r.url.path() == "/auth/cli/verify").count(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn wait_for_completion_times_out_while_pending(#[future(awt)] hub: MockServer) {
        answer_verify(&hub, 401, json!({"reason": "Not attached to a token"}), None).await;

        let err = start(&hub)
            .await
            .wait_for_completion(Duration::from_millis(50), Duration::from_millis(10))
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "Authentication timed out. Please try again.");
    }

    /// The browser login sends Atuin's identity and no credentials, and the code only in the
    /// verify query.
    #[rstest]
    #[tokio::test]
    async fn browser_login_calls_carry_the_identity_headers(#[future(awt)] hub: MockServer) {
        answer_verify(&hub, 401, json!({"reason": "Not attached to a token"}), None).await;

        start(&hub).await.poll().await.unwrap();

        let requests = hub.received_requests().await.unwrap();
        let sent: Vec<_> = requests
            .iter()
            .map(|request| {
                let header = |name: &str| request.headers.get(name).map(|v| v.to_str().unwrap());
                (
                    request.url.path(),
                    request.url.query(),
                    header("user-agent"),
                    header("atuin-version"),
                    header("authorization"),
                    header("api-version"),
                )
            })
            .collect();
        assert_eq!(sent, [
            ("/auth/cli/code", None, Some(ATUIN_USER_AGENT), Some(ATUIN_CARGO_VERSION), None, None),
            (
                "/auth/cli/verify",
                Some("code=s3cret-code"),
                Some(ATUIN_USER_AGENT),
                Some(ATUIN_CARGO_VERSION),
                None,
                None
            ),
        ]);
    }

    #[rstest]
    #[case::linked(200, json!(null), Ok(()))]
    #[case::already_linked(409, json!({"reason": "cli account already linked to a hub account"}), Ok(()))]
    #[case::hub_account_taken(
        400,
        json!({"reason": "hub account already linked to a different cli account"}),
        Err("Hub error: 400 Bad Request - hub account already linked to a different cli account")
    )]
    #[case::unavailable(503, json!(null), Err("Service unavailable: check https://status.atuin.sh"))]
    #[case::proxy_page(
        502,
        json!("<html>Bad Gateway</html>"),
        Err("Hub request failed with status: 502 Bad Gateway")
    )]
    #[tokio::test]
    async fn link_treats_an_existing_link_as_success(
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: Result<(), &str>,
    ) {
        let hub = MockServer::start().await;
        let response = match body {
            Value::Null => ResponseTemplate::new(status),
            body => ResponseTemplate::new(status).set_body_json(body),
        };
        Mock::given(method("POST"))
            .and(path("/api/v0/account/link"))
            .and(wiremock::matchers::header("authorization", "Bearer atapi_hub"))
            .and(wiremock::matchers::body_json(json!({"token": "cli-session"})))
            .respond_with(response)
            .expect(1)
            .mount(&hub)
            .await;
        let api = hub_client(&hub.uri().parse().unwrap(), Some(&SecretString::from("atapi_hub")))
            .unwrap();

        let linked = link(&api, &SecretString::from("cli-session")).await;

        assert_eq!(linked.map_err(|err| err.to_string()), expected.map_err(str::to_owned));
    }
}
