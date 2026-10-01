use std::collections::HashMap;
use std::time::Duration;

use atuin_api_client::{ApiError, AuthToken, MapApiError, types};
use enum_dispatch::enum_dispatch;
use eyre::{Result, bail, eyre};
use reqwest::{StatusCode, Url};
use secrecy::SecretString;

use crate::meta::is_hub_token;
use crate::settings::Settings;

/// Result of an auth operation that may require 2FA.
pub enum AuthResponse {
    /// Operation succeeded; for login/register, contains the session token.
    /// `auth_type` indicates the kind of token: `Some("hub")` for Hub API
    /// tokens (prefixed `atapi_`), `Some("cli")` for legacy CLI session
    /// tokens. `None` when the server didn't include the field (old servers).
    Success {
        session: SecretString,
        auth_type: Option<String>,
    },
    /// Two-factor authentication is required; the caller should prompt for a
    /// TOTP code and retry with it.
    TwoFactorRequired,
}

/// Result of a mutating account operation that may require 2FA.
pub enum MutateResponse {
    /// Operation completed successfully.
    Success,
    /// Two-factor authentication is required; the caller should prompt for a
    /// TOTP code and retry.
    TwoFactorRequired,
}

/// Abstraction over the legacy (Rust sync server) and Hub auth APIs.
///
/// CLI commands use this trait so they don't need to know which backend is
/// active — they just prompt for input and call these methods.
#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait AuthClient: Send + Sync {
    /// Log in with username + password, optionally providing a TOTP code.
    async fn login(
        &self,
        username: &str,
        password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<AuthResponse>;

    /// Register a new account.
    async fn register(
        &self,
        username: &str,
        email: &str,
        password: &SecretString,
    ) -> Result<AuthResponse>;

    /// Change the account password, optionally providing a TOTP code.
    async fn change_password(
        &self,
        current_password: &SecretString,
        new_password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse>;

    /// Delete the account, requiring the current password and optionally a TOTP code.
    async fn delete_account(
        &self,
        password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse>;
}

/// Static-dispatch enum over the two auth backends.
#[enum_dispatch(AuthClient)]
pub enum AnyAuthClient {
    Legacy(LegacyAuthClient),
    Hub(HubAuthClient),
}

/// Resolve the appropriate [`AuthClient`] for the current settings.
pub async fn auth_client(settings: &Settings) -> AnyAuthClient {
    if settings.is_hub_sync() {
        let endpoint = settings.hub_endpoint();
        AnyAuthClient::Hub(HubAuthClient::new(&endpoint, settings.hub_session_token().await.ok()))
    } else {
        AnyAuthClient::Legacy(LegacyAuthClient::new(
            &settings.sync_address,
            settings.session_token().await.ok(),
            settings.network_connect_timeout,
            settings.network_timeout,
            settings.extra_headers.clone(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Legacy backend — talks to the Rust sync server
// ---------------------------------------------------------------------------

pub struct LegacyAuthClient {
    address: Url,
    session_token: Option<SecretString>,
    connect_timeout: Duration,
    timeout: Duration,
    extra_headers: HashMap<String, SecretString>,
}

impl LegacyAuthClient {
    #[must_use]
    pub fn new(
        address: &Url,
        session_token: Option<SecretString>,
        connect_timeout: Duration,
        timeout: Duration,
        extra_headers: HashMap<String, SecretString>,
    ) -> Self {
        Self {
            address: address.clone(),
            session_token,
            connect_timeout,
            timeout,
            extra_headers,
        }
    }

    fn authenticated_api(&self) -> Result<atuin_api_client::Client> {
        let token = self.session_token.clone().ok_or_else(|| eyre!("Not logged in"))?;
        Ok(atuin_api_client::Client::for_sync(
            &self.address,
            &AuthToken::Token(token),
            &self.extra_headers,
            self.connect_timeout,
            self.timeout,
        )?)
    }
}

impl AuthClient for LegacyAuthClient {
    async fn login(
        &self,
        username: &str,
        password: &SecretString,
        _totp_code: Option<&SecretString>,
    ) -> Result<AuthResponse> {
        // The legacy server has no 2FA support; totp_code is ignored.
        let resp = crate::api_client::login(&self.address, username, password, &self.extra_headers)
            .await?;

        Ok(AuthResponse::Success {
            session: resp.session.into(),
            auth_type: resp.auth.or(Some("cli".into())),
        })
    }

    async fn register(
        &self,
        username: &str,
        email: &str,
        password: &SecretString,
    ) -> Result<AuthResponse> {
        let resp = crate::api_client::register(
            &self.address,
            username,
            email,
            password,
            &self.extra_headers,
        )
        .await?;
        Ok(AuthResponse::Success {
            session: resp.session.into(),
            auth_type: resp.auth.or(Some("cli".into())),
        })
    }

    async fn change_password(
        &self,
        current_password: &SecretString,
        new_password: &SecretString,
        _totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse> {
        let body = types::ChangePasswordRequest {
            current_password: current_password.clone().into(),
            new_password: new_password.clone().into(),
            totp_code: None,
        };
        let answer = self.authenticated_api()?.legacy_change_password(&body).map_api_error().await;
        AccountChange::Password.legacy_outcome(answer)
    }

    async fn delete_account(
        &self,
        password: &SecretString,
        _totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse> {
        let body = types::DeleteUserRequest {
            password: password.clone().into(),
            totp_code: None,
        };
        let answer = self.authenticated_api()?.legacy_delete_account(&body).map_api_error().await;
        AccountChange::Deletion.legacy_outcome(answer)
    }
}

// ---------------------------------------------------------------------------
// Hub backend — talks to the Hub v0 API endpoints
// ---------------------------------------------------------------------------

pub struct HubAuthClient {
    address: Url,
    hub_token: Option<SecretString>,
}

impl HubAuthClient {
    #[must_use]
    pub fn new(address: &Url, hub_token: Option<SecretString>) -> Self {
        Self {
            address: address.clone(),
            hub_token,
        }
    }

    fn authenticated_api(&self) -> Result<atuin_api_client::Client> {
        let hub_token = self.hub_token.as_ref().ok_or_else(|| {
            eyre!("Not logged in to Atuin Hub. Please run 'atuin login' to authenticate.")
        })?;

        if !is_hub_token(hub_token) {
            bail!(
                "Your Hub session token is invalid. Please run 'atuin login' to re-authenticate \
                 with Atuin Hub."
            );
        }

        Ok(atuin_api_client::Client::for_hub(&self.address, Some(hub_token))?)
    }
}

impl AuthClient for HubAuthClient {
    async fn login(
        &self,
        username: &str,
        password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<AuthResponse> {
        let body = types::LoginRequest {
            username: username.to_owned(),
            password: password.clone().into(),
            totp_code: totp_code.cloned().map(Into::into),
        };

        match atuin_api_client::Client::for_hub(&self.address, None)?.login(&body).map_api_error().await {
            Ok(resp) => {
                let login = resp.into_inner();
                Ok(AuthResponse::Success {
                    session: login.session.into(),
                    auth_type: login.auth,
                })
            }
            Err(ApiError::Status {
                status: StatusCode::FORBIDDEN,
                code: Some(code),
                ..
            }) if code == "2fa_required" => Ok(AuthResponse::TwoFactorRequired),
            Err(ApiError::Status {
                status: StatusCode::FORBIDDEN,
                reason: Some(reason),
                ..
            }) => bail!("{reason}"),
            Err(ApiError::Status {
                status: StatusCode::UNAUTHORIZED,
                ..
            }) => bail!("invalid credentials"),
            Err(ApiError::Status { status, .. }) => bail!("Hub login failed with status {status}"),
            Err(err @ (ApiError::Transport(_) | ApiError::Decode(_) | ApiError::NotSent(_))) => {
                Err(hub_unanswered(err))
            }
        }
    }

    async fn register(
        &self,
        username: &str,
        email: &str,
        password: &SecretString,
    ) -> Result<AuthResponse> {
        let body = types::RegisterRequest {
            email: email.to_owned(),
            username: username.to_owned(),
            password: password.clone().into(),
        };

        match atuin_api_client::Client::for_hub(&self.address, None)?.register(&body).map_api_error().await {
            Ok(resp) => {
                let reg = resp.into_inner();
                Ok(AuthResponse::Success {
                    session: reg.session.into(),
                    auth_type: reg.auth,
                })
            }
            Err(ApiError::Status {
                reason: Some(reason),
                ..
            }) => bail!("{reason}"),
            Err(ApiError::Status { status, .. }) => {
                bail!("Hub registration failed with status {status}")
            }
            Err(err @ (ApiError::Transport(_) | ApiError::Decode(_) | ApiError::NotSent(_))) => {
                Err(hub_unanswered(err))
            }
        }
    }

    async fn change_password(
        &self,
        current_password: &SecretString,
        new_password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse> {
        let api = self.authenticated_api()?;

        let body = types::ChangePasswordRequest {
            current_password: current_password.clone().into(),
            new_password: new_password.clone().into(),
            totp_code: totp_code.cloned().map(Into::into),
        };
        AccountChange::Password.hub_outcome(api.change_password(&body).map_api_error().await)
    }

    async fn delete_account(
        &self,
        password: &SecretString,
        totp_code: Option<&SecretString>,
    ) -> Result<MutateResponse> {
        let api = self.authenticated_api()?;

        let body = types::DeleteUserRequest {
            password: password.clone().into(),
            totp_code: totp_code.cloned().map(Into::into),
        };
        AccountChange::Deletion.hub_outcome(api.delete_account(&body).map_api_error().await)
    }
}

/// An account change the server guards with the account's password.
#[derive(Debug, Clone, Copy)]
enum AccountChange {
    Password,
    Deletion,
}

impl AccountChange {
    /// The message for a 401 that gives no reason of its own.
    const fn wrong_password(self) -> &'static str {
        match self {
            Self::Password => "current password is incorrect",
            Self::Deletion => "password is incorrect",
        }
    }

    /// The change as the hub's failure message names it, e.g. `password change`.
    const fn name(self) -> &'static str {
        match self {
            Self::Password => "password change",
            Self::Deletion => "account deletion",
        }
    }

    /// The outcome of the legacy server's `answer`, which succeeded only on a 200.
    fn legacy_outcome<T>(self, answer: Result<T, ApiError>) -> Result<MutateResponse> {
        match answer {
            // A 200 whose body is not the API's still made the change.
            Ok(_) | Err(ApiError::Decode(_)) => Ok(MutateResponse::Success),
            Err(ApiError::Status {
                status: StatusCode::UNAUTHORIZED,
                ..
            }) => bail!("{}", self.wrong_password()),
            Err(ApiError::Status {
                status: StatusCode::FORBIDDEN,
                ..
            }) => bail!("invalid login details"),
            Err(ApiError::Status { .. }) => bail!("unknown error"),
            Err(err @ (ApiError::Transport(_) | ApiError::NotSent(_))) => Err(err.into()),
        }
    }

    /// The outcome of the hub's `answer`, which succeeded on any 2xx, and whose error `code` asks
    /// for or rejects a TOTP code.
    fn hub_outcome<T>(self, answer: Result<T, ApiError>) -> Result<MutateResponse> {
        let (status, reason, code) = match answer {
            // A 2xx other than the documented 200, or one whose body is not the API's, still made
            // the change.
            Ok(_) | Err(ApiError::Decode(_)) => return Ok(MutateResponse::Success),
            Err(ApiError::Status { status, .. }) if status.is_success() => {
                return Ok(MutateResponse::Success);
            }
            Err(ApiError::Status {
                status,
                reason,
                code,
                ..
            }) => (status, reason, code),
            Err(err @ (ApiError::Transport(_) | ApiError::NotSent(_))) => {
                return Err(hub_unanswered(err));
            }
        };

        match (code.as_deref(), reason, status) {
            (Some("2fa_required"), ..) => Ok(MutateResponse::TwoFactorRequired),
            (Some("invalid_2fa_code"), ..) => bail!("invalid two-factor code"),
            (_, Some(reason), _) => bail!("{reason}"),
            (_, None, StatusCode::UNAUTHORIZED) => bail!("{}", self.wrong_password()),
            (_, None, StatusCode::FORBIDDEN) => bail!("invalid login details"),
            (_, None, status) => bail!("Hub {} failed with status {status}", self.name()),
        }
    }
}

/// The report for a hub call that failed without an error status.
fn hub_unanswered(err: ApiError) -> eyre::Report {
    match err {
        ApiError::Transport(_) | ApiError::NotSent(_) => {
            eyre::Report::new(err).wrap_err("failed to connect to Atuin Hub")
        }
        ApiError::Status { .. } | ApiError::Decode(_) => err.into(),
    }
}
