use std::collections::HashMap;
use std::time::Duration;

use atuin_api_client::{ApiError, MapApiError, types};
use enum_dispatch::enum_dispatch;
use eyre::{Result, bail, eyre};
use reqwest::{StatusCode, Url};
use secrecy::SecretString;

use crate::api_client::{AuthToken, authenticated_http};
use crate::http::hub_client;
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
        let http = authenticated_http(
            &AuthToken::Token(token),
            self.connect_timeout,
            self.timeout,
            &self.extra_headers,
        )?;
        Ok(atuin_api_client::Client::from_http(&self.address, http)?)
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

        hub_client(&self.address, Some(hub_token))
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

        match hub_client(&self.address, None)?.login(&body).map_api_error().await {
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

        match hub_client(&self.address, None)?.register(&body).map_api_error().await {
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

#[cfg(test)]
mod tests {
    use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_USER_AGENT};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use secrecy::ExposeSecret;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// What a login or registration came to, e.g. `session atapi_s (Some("hub"))`.
    fn signed_in(response: Result<AuthResponse>) -> String {
        match response {
            Ok(AuthResponse::Success { session, auth_type }) => {
                format!("session {} ({auth_type:?})", session.expose_secret())
            }
            Ok(AuthResponse::TwoFactorRequired) => "2fa required".to_owned(),
            Err(err) => format!("error: {err}"),
        }
    }

    /// What an account change came to.
    fn changed(response: Result<MutateResponse>) -> String {
        match response {
            Ok(MutateResponse::Success) => "changed".to_owned(),
            Ok(MutateResponse::TwoFactorRequired) => "2fa required".to_owned(),
            Err(err) => format!("error: {err}"),
        }
    }

    /// An answer with `status`, `body` (as text when it is a string, none when null), and
    /// `version` as `Atuin-Version`.
    fn answer(status: u16, body: Value, version: Option<&str>) -> ResponseTemplate {
        let response = ResponseTemplate::new(status);
        let response = match version {
            Some(version) => response.insert_header("atuin-version", version),
            None => response,
        };
        match body {
            Value::Null => response,
            Value::String(text) => response.set_body_string(text),
            body => response.set_body_json(body),
        }
    }

    /// A server answering `verb route` with `response`.
    async fn serve(verb: &str, route: &str, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method(verb)).and(path(route)).respond_with(response).mount(&server).await;
        server
    }

    async fn change(client: &impl AuthClient, change: AccountChange) -> Result<MutateResponse> {
        let totp = SecretString::from("123456");
        match change {
            AccountChange::Password => {
                client.change_password(&"old".into(), &"new".into(), Some(&totp)).await
            }
            AccountChange::Deletion => client.delete_account(&"pw".into(), Some(&totp)).await,
        }
    }

    const fn hub_route(change: AccountChange) -> (&'static str, &'static str) {
        match change {
            AccountChange::Password => ("PATCH", "/api/v0/account/password"),
            AccountChange::Deletion => ("DELETE", "/api/v0/account"),
        }
    }

    const fn legacy_route(change: AccountChange) -> (&'static str, &'static str) {
        match change {
            AccountChange::Password => ("PATCH", "/account/password"),
            AccountChange::Deletion => ("DELETE", "/account"),
        }
    }

    fn hub(server: &MockServer, token: Option<&str>) -> HubAuthClient {
        HubAuthClient::new(&server.uri().parse().unwrap(), token.map(SecretString::from))
    }

    fn legacy(server: &MockServer, session: Option<&str>) -> LegacyAuthClient {
        LegacyAuthClient::new(
            &server.uri().parse().unwrap(),
            session.map(SecretString::from),
            Duration::from_secs(5),
            Duration::from_secs(30),
            HashMap::from([("X-Extra".to_owned(), SecretString::from("extra"))]),
        )
    }

    /// The headers of the only request `server` received, by name, `None` for an absent one.
    async fn sent_headers<const N: usize>(
        server: &MockServer,
        names: [&str; N],
    ) -> [Option<String>; N] {
        let requests = server.received_requests().await.unwrap();
        let [request] = requests.as_slice() else {
            panic!("expected one request, got {requests:?}");
        };
        names.map(|name| request.headers.get(name).map(|v| v.to_str().unwrap().to_owned()))
    }

    async fn sent_body(server: &MockServer) -> Value {
        let requests = server.received_requests().await.unwrap();
        serde_json::from_slice(&requests[0].body).unwrap()
    }

    #[rstest]
    #[case::session(200, json!({"session": "atapi_s", "auth": "hub"}), r#"session atapi_s (Some("hub"))"#)]
    #[case::needs_totp(
        403,
        json!({"reason": "two-factor authentication required", "code": "2fa_required"}),
        "2fa required"
    )]
    #[case::refused(403, json!({"reason": "account not migrated to hub"}), "error: account not migrated to hub")]
    #[case::wrong_totp(
        401,
        json!({"reason": "invalid two-factor code", "code": "invalid_2fa_code"}),
        "error: invalid credentials"
    )]
    #[case::bare_403(403, Value::Null, "error: Hub login failed with status 403 Forbidden")]
    #[case::proxy_page(
        403,
        json!("<html>403 Forbidden</html>"),
        "error: Hub login failed with status 403 Forbidden"
    )]
    #[case::server_error(
        500,
        Value::Null,
        "error: Hub login failed with status 500 Internal Server Error"
    )]
    #[tokio::test]
    async fn hub_login_asks_for_totp_on_2fa_required(
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let server = serve("POST", "/api/v0/login", answer(status, body, None)).await;

        let response = hub(&server, None).login("ellie", &"pw".into(), None).await;

        assert_eq!(signed_in(response), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn hub_login_sends_the_totp_code_without_credentials() {
        let response = answer(200, json!({"session": "atapi_s"}), None);
        let server = serve("POST", "/api/v0/login", response).await;

        hub(&server, Some("atapi_t"))
            .login("ellie", &"pw".into(), Some(&"123456".into()))
            .await
            .unwrap();

        assert_eq!(
            sent_body(&server).await,
            json!({"username": "ellie", "password": "pw", "totp_code": "123456"})
        );
        assert_eq!(
            sent_headers(&server, ["user-agent", "atuin-version", "authorization", "api-version"])
                .await,
            [Some(ATUIN_USER_AGENT.to_owned()), Some(ATUIN_CARGO_VERSION.to_owned()), None, None]
        );
    }

    #[rstest]
    #[tokio::test]
    async fn hub_login_reports_an_unreachable_hub() {
        let client = HubAuthClient::new(&"http://127.0.0.1:1".parse().unwrap(), None);

        let response = client.login("ellie", &"pw".into(), None).await;

        assert_eq!(signed_in(response), "error: failed to connect to Atuin Hub");
    }

    #[rstest]
    #[case::session(200, json!({"session": "atapi_s", "auth": "hub"}), r#"session atapi_s (Some("hub"))"#)]
    #[case::invalid(400, json!({"reason": "email has invalid format"}), "error: email has invalid format")]
    #[case::bare_error(
        500,
        Value::Null,
        "error: Hub registration failed with status 500 Internal Server Error"
    )]
    #[case::proxy_page(
        502,
        json!("<html><body><h1>502 Bad Gateway</h1>cloudflare</body></html>"),
        "error: Hub registration failed with status 502 Bad Gateway"
    )]
    #[tokio::test]
    async fn hub_register_reports_the_reason(
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let server = serve("POST", "/api/v0/register", answer(status, body, None)).await;

        let response = hub(&server, None).register("ellie", "e@example.com", &"pw".into()).await;

        assert_eq!(signed_in(response), expected);
    }

    #[rstest]
    #[case::changed(AccountChange::Password, 200, json!({}), "changed")]
    #[case::deleted(AccountChange::Deletion, 200, json!({}), "changed")]
    #[case::bodyless_200(AccountChange::Password, 200, Value::Null, "changed")]
    #[case::no_content(AccountChange::Deletion, 204, Value::Null, "changed")]
    #[case::needs_totp(
        AccountChange::Password,
        403,
        json!({"reason": "two-factor authentication required", "code": "2fa_required"}),
        "2fa required"
    )]
    #[case::wrong_totp(
        AccountChange::Deletion,
        401,
        json!({"reason": "invalid two-factor code", "code": "invalid_2fa_code"}),
        "error: invalid two-factor code"
    )]
    #[case::reason(
        AccountChange::Password,
        401,
        json!({"reason": "password is not correct"}),
        "error: password is not correct"
    )]
    #[case::bare_401_password(
        AccountChange::Password,
        401,
        Value::Null,
        "error: current password is incorrect"
    )]
    #[case::bare_401_deletion(
        AccountChange::Deletion,
        401,
        Value::Null,
        "error: password is incorrect"
    )]
    #[case::bare_403(AccountChange::Deletion, 403, Value::Null, "error: invalid login details")]
    #[case::bare_500_password(
        AccountChange::Password,
        500,
        Value::Null,
        "error: Hub password change failed with status 500 Internal Server Error"
    )]
    #[case::bare_500_deletion(
        AccountChange::Deletion,
        500,
        Value::Null,
        "error: Hub account deletion failed with status 500 Internal Server Error"
    )]
    #[case::proxy_page(
        AccountChange::Password,
        502,
        json!("<html>502 Bad Gateway</html>"),
        "error: Hub password change failed with status 502 Bad Gateway"
    )]
    #[tokio::test]
    async fn hub_account_changes_read_the_error_code(
        #[case] account_change: AccountChange,
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let (verb, route) = hub_route(account_change);
        let server = serve(verb, route, answer(status, body, None)).await;

        let response = change(&hub(&server, Some("atapi_t")), account_change).await;

        assert_eq!(changed(response), expected);
    }

    #[rstest]
    #[case::password(
        AccountChange::Password,
        json!({"current_password": "old", "new_password": "new", "totp_code": "123456"})
    )]
    #[case::deletion(AccountChange::Deletion, json!({"password": "pw", "totp_code": "123456"}))]
    #[tokio::test]
    async fn hub_account_changes_send_the_hub_token(
        #[case] account_change: AccountChange,
        #[case] body: Value,
    ) {
        let (verb, route) = hub_route(account_change);
        let server = serve(verb, route, answer(200, json!({}), None)).await;

        change(&hub(&server, Some("atapi_t")), account_change).await.unwrap();

        assert_eq!(sent_body(&server).await, body);
        assert_eq!(
            sent_headers(&server, ["authorization", "user-agent", "atuin-version", "api-version"])
                .await,
            [
                Some("Bearer atapi_t".to_owned()),
                Some(ATUIN_USER_AGENT.to_owned()),
                Some(ATUIN_CARGO_VERSION.to_owned()),
                None
            ]
        );
    }

    #[rstest]
    #[case::logged_out(
        None,
        "error: Not logged in to Atuin Hub. Please run 'atuin login' to authenticate."
    )]
    #[case::cli_session(
        Some("cli-session"),
        "error: Your Hub session token is invalid. Please run 'atuin login' to re-authenticate \
         with Atuin Hub."
    )]
    #[tokio::test]
    async fn hub_account_changes_need_a_hub_token(
        #[values(AccountChange::Password, AccountChange::Deletion)] account_change: AccountChange,
        #[case] token: Option<&str>,
        #[case] expected: &str,
    ) {
        let server = MockServer::start().await;

        let response = change(&hub(&server, token), account_change).await;

        assert_eq!(changed(response), expected);
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    #[rstest]
    #[case::changed(AccountChange::Password, 200, json!({}), "changed")]
    #[case::deleted(AccountChange::Deletion, 200, json!({}), "changed")]
    #[case::wrong_password_change(
        AccountChange::Password,
        401,
        json!({"reason": "password is not correct"}),
        "error: current password is incorrect"
    )]
    #[case::wrong_password_deletion(
        AccountChange::Deletion,
        401,
        json!({"reason": "password is not correct"}),
        "error: password is incorrect"
    )]
    #[case::unknown_session(
        AccountChange::Password,
        403,
        json!({"reason": "session not found"}),
        "error: invalid login details"
    )]
    #[case::bodyless_200(AccountChange::Password, 200, Value::Null, "changed")]
    #[case::other_success(AccountChange::Deletion, 204, Value::Null, "error: unknown error")]
    #[case::server_error(
        AccountChange::Password,
        500,
        json!({"reason": "failed to change user password"}),
        "error: unknown error"
    )]
    #[tokio::test]
    async fn legacy_account_changes_succeed_only_on_200(
        #[case] account_change: AccountChange,
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let (verb, route) = legacy_route(account_change);
        let server = serve(verb, route, answer(status, body, Some(ATUIN_CARGO_VERSION))).await;

        let response = change(&legacy(&server, Some("sess")), account_change).await;

        assert_eq!(changed(response), expected);
    }

    #[rstest]
    #[case::password(
        AccountChange::Password,
        json!({"current_password": "old", "new_password": "new"})
    )]
    #[case::deletion(AccountChange::Deletion, json!({"password": "pw"}))]
    #[tokio::test]
    async fn legacy_account_changes_send_the_session_and_extra_headers(
        #[case] account_change: AccountChange,
        #[case] body: Value,
    ) {
        let (verb, route) = legacy_route(account_change);
        let server = serve(verb, route, answer(200, json!({}), Some(ATUIN_CARGO_VERSION))).await;

        change(&legacy(&server, Some("sess")), account_change).await.unwrap();

        assert_eq!(sent_body(&server).await, body);
        assert_eq!(
            sent_headers(&server, [
                "authorization",
                "x-extra",
                "user-agent",
                "atuin-version",
                "api-version",
            ])
            .await,
            [
                Some("Token sess".to_owned()),
                Some("extra".to_owned()),
                Some(ATUIN_USER_AGENT.to_owned()),
                Some(ATUIN_CARGO_VERSION.to_owned()),
                None
            ]
        );
    }

    #[rstest]
    #[tokio::test]
    async fn legacy_account_changes_need_a_session(
        #[values(AccountChange::Password, AccountChange::Deletion)] account_change: AccountChange,
    ) {
        let server = MockServer::start().await;

        let response = change(&legacy(&server, None), account_change).await;

        assert_eq!(changed(response), "error: Not logged in");
    }

    #[rstest]
    #[case::current(Some(ATUIN_CARGO_VERSION), r#"session sess (Some("cli"))"#)]
    #[case::older_major(Some("1.0.0"), "error: Could not login due to version mismatch")]
    #[case::unversioned(
        None,
        "error: Server not reporting its version: it is either too old or unhealthy"
    )]
    #[tokio::test]
    async fn legacy_login_checks_the_server_version(
        #[case] version: Option<&str>,
        #[case] expected: &str,
    ) {
        let server =
            serve("POST", "/login", answer(200, json!({"session": "sess"}), version)).await;

        let response = legacy(&server, None).login("ellie", &"pw".into(), None).await;

        assert_eq!(signed_in(response), expected);
        assert_eq!(sent_body(&server).await, json!({"username": "ellie", "password": "pw"}));
    }

    #[rstest]
    #[case::taken(200, json!({"username": "ellie"}), 0, "error: username already in use")]
    #[case::taken_by_an_odd_answer(200, json!({}), 0, "error: username already in use")]
    #[case::free(404, json!({"reason": "user not found"}), 1, r#"session sess (Some("cli"))"#)]
    #[tokio::test]
    async fn legacy_register_checks_the_username_first(
        #[case] lookup_status: u16,
        #[case] lookup_body: Value,
        #[case] registrations: u64,
        #[case] expected: &str,
    ) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/ellie"))
            .respond_with(answer(lookup_status, lookup_body, Some(ATUIN_CARGO_VERSION)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(answer(200, json!({"session": "sess"}), Some(ATUIN_CARGO_VERSION)))
            .expect(registrations)
            .mount(&server)
            .await;

        let response = legacy(&server, None).register("ellie", "e@example.com", &"pw".into()).await;

        assert_eq!(signed_in(response), expected);
    }

    #[derive(Debug, Clone, Copy)]
    enum SignIn {
        Login,
        Register,
    }

    async fn sign_in(client: &impl AuthClient, how: SignIn) -> Result<AuthResponse> {
        match how {
            SignIn::Login => client.login("ellie", &"pw".into(), None).await,
            SignIn::Register => client.register("ellie", "e@example.com", &"pw".into()).await,
        }
    }

    /// A legacy server with no account `ellie` that signs anyone in, reporting `version`.
    async fn legacy_server(version: Option<&str>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/ellie"))
            .respond_with(answer(404, json!({"reason": "user not found"}), version))
            .mount(&server)
            .await;
        for route in ["/login", "/register"] {
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(answer(200, json!({"session": "sess"}), version))
                .mount(&server)
                .await;
        }
        server
    }

    /// Signing in to a legacy server sends the user's `extra_headers` and Atuin's identity on
    /// every call, and no credentials or `api-version`.
    #[rstest]
    #[case::login(SignIn::Login, &["/login"])]
    #[case::register(SignIn::Register, &["/user/ellie", "/register"])]
    #[tokio::test]
    async fn legacy_sign_in_sends_the_user_and_identity_headers(
        #[case] how: SignIn,
        #[case] paths: &[&str],
    ) {
        let server = legacy_server(Some(ATUIN_CARGO_VERSION)).await;

        sign_in(&legacy(&server, None), how).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let sent: Vec<_> = requests
            .iter()
            .map(|request| {
                let header = |name: &str| request.headers.get(name).map(|v| v.to_str().unwrap());
                (
                    request.url.path(),
                    header("x-extra"),
                    header("user-agent"),
                    header("atuin-version"),
                    header("authorization"),
                    header("api-version"),
                )
            })
            .collect();
        let expected: Vec<_> = paths
            .iter()
            .map(|&path| {
                (path, Some("extra"), Some(ATUIN_USER_AGENT), Some(ATUIN_CARGO_VERSION), None, None)
            })
            .collect();
        assert_eq!(sent, expected);
    }

    #[rstest]
    #[case::login_older_major(
        SignIn::Login,
        Some("1.0.0"),
        "Could not login due to version mismatch"
    )]
    #[case::register_older_major(
        SignIn::Register,
        Some("1.0.0"),
        "could not register user due to version mismatch"
    )]
    #[case::register_unversioned(
        SignIn::Register,
        None,
        "Server not reporting its version: it is either too old or unhealthy"
    )]
    #[tokio::test]
    async fn legacy_sign_in_refuses_an_older_server(
        #[case] how: SignIn,
        #[case] version: Option<&str>,
        #[case] expected: &str,
    ) {
        let server = legacy_server(version).await;

        let response = sign_in(&legacy(&server, None), how).await;

        assert_eq!(signed_in(response), format!("error: {expected}"));
    }

    /// A `\` would read as `/` in the lookup's path, so the server's own refusal answers it.
    #[rstest]
    #[tokio::test]
    async fn legacy_register_leaves_a_backslash_to_the_server() {
        let server = serve(
            "POST",
            "/register",
            answer(
                400,
                json!({"reason": "Only alphanumeric and hyphens (-) are allowed in usernames"}),
                Some(ATUIN_CARGO_VERSION),
            ),
        )
        .await;

        let response = legacy(&server, None).register("..\\", "e@example.com", &"pw".into()).await;

        assert!(signed_in(response).ends_with(
            "/register, 400 Bad Request - Only alphanumeric and hyphens (-) are allowed in \
             usernames."
        ),);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(), ["/register"]);
    }

    #[rstest]
    #[tokio::test]
    async fn legacy_register_rejects_dot_usernames(#[values(".", "..")] username: &str) {
        let server = MockServer::start().await;

        let response =
            legacy(&server, None).register(username, "e@example.com", &"pw".into()).await;

        assert_eq!(signed_in(response), "error: path segments cannot be . or ..");
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }
}
