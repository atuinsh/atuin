use async_trait::async_trait;
use eyre::{Context, Result, bail};
use reqwest::{StatusCode, Url, header::USER_AGENT};
use serde::Deserialize;

use atuin_common::{
    api::{
        ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ChangePasswordRequest, LoginRequest,
        LoginResponse, RegisterResponse,
    },
    tls::ensure_crypto_provider,
};

use crate::settings::Settings;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

/// Result of an auth operation that may require 2FA.
pub enum AuthResponse {
    /// Operation succeeded; for login/register, contains the session token.
    /// `auth_type` indicates the kind of token: `Some("hub")` for Hub API
    /// tokens (prefixed `atapi_`), `Some("cli")` for legacy CLI session
    /// tokens. `None` when the server didn't include the field (old servers).
    Success {
        session: String,
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
#[async_trait]
pub trait AuthClient: Send + Sync {
    /// Log in with username + password, optionally providing a TOTP code.
    async fn login(
        &self,
        username: &str,
        password: &str,
        totp_code: Option<&str>,
    ) -> Result<AuthResponse>;

    /// Register a new account.
    async fn register(&self, username: &str, email: &str, password: &str) -> Result<AuthResponse>;

    /// Change the account password, optionally providing a TOTP code.
    async fn change_password(
        &self,
        current_password: &str,
        new_password: &str,
        totp_code: Option<&str>,
    ) -> Result<MutateResponse>;

    /// Delete the account, requiring the current password and optionally a TOTP code.
    async fn delete_account(
        &self,
        password: &str,
        totp_code: Option<&str>,
    ) -> Result<MutateResponse>;
}

/// Resolve the appropriate [`AuthClient`] for the current settings.
pub async fn auth_client(settings: &Settings) -> Box<dyn AuthClient> {
    if settings.is_hub_sync() {
        let endpoint = settings.active_hub_endpoint().unwrap_or_default();
        Box::new(HubAuthClient::new(
            endpoint.as_ref(),
            settings.hub_session_token().await.ok(),
        )) as Box<dyn AuthClient>
    } else {
        Box::new(LegacyAuthClient::new(
            &settings.sync_address,
            settings.session_token().await.ok(),
            settings.network_connect_timeout,
            settings.network_timeout,
        )) as Box<dyn AuthClient>
    }
}

// ---------------------------------------------------------------------------
// Legacy backend — talks to the Rust sync server
// ---------------------------------------------------------------------------

pub struct LegacyAuthClient {
    address: String,
    session_token: Option<String>,
    connect_timeout: u64,
    timeout: u64,
}

impl LegacyAuthClient {
    pub fn new(
        address: &str,
        session_token: Option<String>,
        connect_timeout: u64,
        timeout: u64,
    ) -> Self {
        Self {
            address: address.to_string(),
            session_token,
            connect_timeout,
            timeout,
        }
    }

    fn authenticated_client(&self) -> Result<reqwest::Client> {
        let token = self
            .session_token
            .as_deref()
            .ok_or_else(|| eyre::eyre!("Not logged in"))?;

        ensure_crypto_provider();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Token {token}").parse()?,
        );
        headers.insert(USER_AGENT, APP_USER_AGENT.parse()?);
        headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

        Ok(reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(std::time::Duration::new(self.connect_timeout, 0))
            .timeout(std::time::Duration::new(self.timeout, 0))
            .build()?)
    }
}

#[async_trait]
impl AuthClient for LegacyAuthClient {
    async fn login(
        &self,
        username: &str,
        password: &str,
        _totp_code: Option<&str>,
    ) -> Result<AuthResponse> {
        // The legacy server has no 2FA support; totp_code is ignored.
        let resp = crate::api_client::login(
            &self.address,
            LoginRequest {
                username: username.to_string(),
                password: password.to_string(),
            },
        )
        .await?;

        Ok(AuthResponse::Success {
            session: resp.session,
            auth_type: resp.auth.or(Some("cli".into())),
        })
    }

    async fn register(&self, username: &str, email: &str, password: &str) -> Result<AuthResponse> {
        let resp = crate::api_client::register(&self.address, username, email, password).await?;
        Ok(AuthResponse::Success {
            session: resp.session,
            auth_type: resp.auth.or(Some("cli".into())),
        })
    }

    async fn change_password(
        &self,
        current_password: &str,
        new_password: &str,
        _totp_code: Option<&str>,
    ) -> Result<MutateResponse> {
        let client = self.authenticated_client()?;
        let url = make_url(&self.address, "/account/password")?;

        let resp = client
            .patch(&url)
            .json(&ChangePasswordRequest {
                current_password: current_password.to_string(),
                new_password: new_password.to_string(),
            })
            .send()
            .await?;

        match resp.status().as_u16() {
            200 => Ok(MutateResponse::Success),
            401 => bail!("current password is incorrect"),
            403 => bail!("invalid login details"),
            _ => bail!("unknown error"),
        }
    }

    async fn delete_account(
        &self,
        password: &str,
        _totp_code: Option<&str>,
    ) -> Result<MutateResponse> {
        let client = self.authenticated_client()?;
        let url = make_url(&self.address, "/account")?;

        let resp = client
            .delete(&url)
            .json(&serde_json::json!({ "password": password }))
            .send()
            .await?;

        match resp.status().as_u16() {
            200 => Ok(MutateResponse::Success),
            401 => bail!("password is incorrect"),
            403 => bail!("invalid login details"),
            _ => bail!("unknown error"),
        }
    }
}

// ---------------------------------------------------------------------------
// Hub backend — talks to the Hub v0 API endpoints
// ---------------------------------------------------------------------------

pub struct HubAuthClient {
    address: String,
    hub_token: Option<String>,
}

impl HubAuthClient {
    pub fn new(address: &str, hub_token: Option<String>) -> Self {
        Self {
            address: address.trim_end_matches('/').to_string(),
            hub_token,
        }
    }
}

/// Hub v0 error/status response — includes an optional `code` field for
/// machine-readable status like `"2fa_required"`.
#[derive(Debug, Deserialize)]
struct HubErrorResponse {
    reason: String,
    code: Option<String>,
}

#[async_trait]
impl AuthClient for HubAuthClient {
    async fn login(
        &self,
        username: &str,
        password: &str,
        totp_code: Option<&str>,
    ) -> Result<AuthResponse> {
        ensure_crypto_provider();
        let url = make_url(&self.address, "/api/v0/login")?;
        let client = reqwest::Client::new();

        let mut body = serde_json::json!({
            "username": username,
            "password": password,
        });
        if let Some(code) = totp_code {
            body["totp_code"] = serde_json::Value::String(code.to_string());
        }

        let resp = client
            .post(&url)
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
            .json(&body)
            .send()
            .await
            .context("failed to connect to Atuin Hub")?;

        let status = resp.status();

        if status.is_success() {
            let login: LoginResponse = resp.json().await?;
            return Ok(AuthResponse::Success {
                session: login.session,
                auth_type: login.auth,
            });
        }

        if status == StatusCode::FORBIDDEN
            && let Ok(err) = resp.json::<HubErrorResponse>().await
        {
            if err.code.as_deref() == Some("2fa_required") {
                return Ok(AuthResponse::TwoFactorRequired);
            }
            bail!("{}", err.reason);
        }

        if status == StatusCode::UNAUTHORIZED {
            bail!("invalid credentials");
        }

        bail!("Hub login failed with status {status}");
    }

    async fn register(&self, username: &str, email: &str, password: &str) -> Result<AuthResponse> {
        ensure_crypto_provider();
        let url = make_url(&self.address, "/api/v0/register")?;
        let client = reqwest::Client::new();

        let resp = client
            .post(&url)
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
            .json(&serde_json::json!({
                "email": email,
                "username": username,
                "password": password,
            }))
            .send()
            .await
            .context("failed to connect to Atuin Hub")?;

        let status = resp.status();

        if status.is_success() {
            let reg: RegisterResponse = resp.json().await?;
            return Ok(AuthResponse::Success {
                session: reg.session,
                auth_type: reg.auth,
            });
        }

        if let Ok(err) = resp.json::<HubErrorResponse>().await {
            bail!("{}", err.reason);
        }

        bail!("Hub registration failed with status {status}");
    }

    async fn change_password(
        &self,
        current_password: &str,
        new_password: &str,
        totp_code: Option<&str>,
    ) -> Result<MutateResponse> {
        let hub_token = self.hub_token.as_deref().ok_or_else(|| {
            eyre::eyre!(
                "Not logged in to Atuin Hub. \
                     Please run 'atuin login' to authenticate."
            )
        })?;

        if !hub_token.starts_with("atapi_") {
            bail!(
                "Your Hub session token is invalid. \
                 Please run 'atuin login' to re-authenticate with Atuin Hub."
            );
        }

        ensure_crypto_provider();
        let url = make_url(&self.address, "/api/v0/account/password")?;
        let client = reqwest::Client::new();

        let mut body = serde_json::json!({
            "current_password": current_password,
            "new_password": new_password,
        });
        if let Some(code) = totp_code {
            body["totp_code"] = serde_json::Value::String(code.to_string());
        }

        let resp = client
            .patch(&url)
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
            .bearer_auth(hub_token)
            .json(&body)
            .send()
            .await
            .context("failed to connect to Atuin Hub")?;

        let status = resp.status();

        if status.is_success() {
            return Ok(MutateResponse::Success);
        }

        if let Ok(err) = resp.json::<HubErrorResponse>().await {
            match err.code.as_deref() {
                Some("2fa_required") => return Ok(MutateResponse::TwoFactorRequired),
                Some("invalid_2fa_code") => bail!("invalid two-factor code"),
                _ => bail!("{}", err.reason),
            }
        }

        match status {
            StatusCode::UNAUTHORIZED => bail!("current password is incorrect"),
            StatusCode::FORBIDDEN => bail!("invalid login details"),
            _ => bail!("Hub password change failed with status {status}"),
        }
    }

    async fn delete_account(
        &self,
        password: &str,
        totp_code: Option<&str>,
    ) -> Result<MutateResponse> {
        let hub_token = self.hub_token.as_deref().ok_or_else(|| {
            eyre::eyre!(
                "Not logged in to Atuin Hub. \
                     Please run 'atuin login' to authenticate."
            )
        })?;

        if !hub_token.starts_with("atapi_") {
            bail!(
                "Your Hub session token is invalid. \
                 Please run 'atuin login' to re-authenticate with Atuin Hub."
            );
        }

        ensure_crypto_provider();
        let url = make_url(&self.address, "/api/v0/account")?;
        let client = reqwest::Client::new();

        let mut body = serde_json::json!({
            "password": password,
        });
        if let Some(code) = totp_code {
            body["totp_code"] = serde_json::Value::String(code.to_string());
        }

        let resp = client
            .delete(&url)
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
            .bearer_auth(hub_token)
            .json(&body)
            .send()
            .await
            .context("failed to connect to Atuin Hub")?;

        let status = resp.status();

        if status.is_success() {
            return Ok(MutateResponse::Success);
        }

        if let Ok(err) = resp.json::<HubErrorResponse>().await {
            match err.code.as_deref() {
                Some("2fa_required") => return Ok(MutateResponse::TwoFactorRequired),
                Some("invalid_2fa_code") => bail!("invalid two-factor code"),
                _ => bail!("{}", err.reason),
            }
        }

        match status {
            StatusCode::UNAUTHORIZED => bail!("password is incorrect"),
            StatusCode::FORBIDDEN => bail!("invalid login details"),
            _ => bail!("Hub account deletion failed with status {status}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_url(address: &str, path: &str) -> Result<String> {
    let address = if address.ends_with('/') {
        address.to_string()
    } else {
        format!("{address}/")
    };

    let path = path.strip_prefix('/').unwrap_or(path);

    let url = Url::parse(&address)
        .context("failed to parse server address")?
        .join(path)
        .context("failed to join URL path")?;

    Ok(url.to_string())
}
