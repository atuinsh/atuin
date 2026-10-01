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
    use wiremock::matchers::{body_json, header, method, path};
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

    /// The hub request a `change` makes: its verb, route and body.
    fn hub_request(change: AccountChange) -> (&'static str, &'static str, Value) {
        match change {
            AccountChange::Password => (
                "PATCH",
                "/api/v0/account/password",
                json!({"current_password": "old", "new_password": "new", "totp_code": "123456"}),
            ),
            AccountChange::Deletion => {
                ("DELETE", "/api/v0/account", json!({"password": "pw", "totp_code": "123456"}))
            }
        }
    }

    /// The legacy request a `change` makes: its verb, route and body.
    fn legacy_request(change: AccountChange) -> (&'static str, &'static str, Value) {
        match change {
            AccountChange::Password => (
                "PATCH",
                "/account/password",
                json!({"current_password": "old", "new_password": "new"}),
            ),
            AccountChange::Deletion => ("DELETE", "/account", json!({"password": "pw"})),
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

    async fn sent_body(server: &MockServer) -> Value {
        let requests = server.received_requests().await.unwrap();
        serde_json::from_slice(&requests[0].body).unwrap()
    }

    #[rstest]
    #[case::needs_totp(
        403,
        json!({"reason": "two-factor authentication required", "code": "2fa_required"}),
        "2fa required"
    )]
    #[case::refused(403, json!({"reason": "account not migrated to hub"}), "error: account not migrated to hub")]
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
    async fn hub_login_sends_the_totp_code() {
        let response = answer(200, json!({"session": "atapi_s"}), None);
        let server = serve("POST", "/api/v0/login", response).await;

        hub(&server, None).login("ellie", &"pw".into(), Some(&"123456".into())).await.unwrap();

        assert_eq!(
            sent_body(&server).await,
            json!({"username": "ellie", "password": "pw", "totp_code": "123456"})
        );
    }

    #[rstest]
    #[tokio::test]
    async fn hub_register_reports_the_reason() {
        let response = answer(400, json!({"reason": "email has invalid format"}), None);
        let server = serve("POST", "/api/v0/register", response).await;

        let response = hub(&server, None).register("ellie", "e@example.com", &"pw".into()).await;

        assert_eq!(signed_in(response), "error: email has invalid format");
    }

    #[rstest]
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
        json!({"reason": "code expired", "code": "invalid_2fa_code"}),
        "error: invalid two-factor code"
    )]
    #[case::reason(
        AccountChange::Password,
        401,
        json!({"reason": "password is not correct"}),
        "error: password is not correct"
    )]
    #[tokio::test]
    async fn hub_account_changes_read_the_error_code(
        #[case] account_change: AccountChange,
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let (verb, route, request) = hub_request(account_change);
        let server = MockServer::start().await;
        Mock::given(method(verb))
            .and(path(route))
            .and(header("authorization", "Bearer atapi_t"))
            .and(body_json(request))
            .respond_with(answer(status, body, None))
            .expect(1)
            .mount(&server)
            .await;

        let response = change(&hub(&server, Some("atapi_t")), account_change).await;

        assert_eq!(changed(response), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn hub_account_changes_need_a_hub_token(
        #[values(AccountChange::Password, AccountChange::Deletion)] account_change: AccountChange,
    ) {
        let server = MockServer::start().await;

        let response = change(&hub(&server, Some("cli-session")), account_change).await;

        assert_eq!(
            changed(response),
            "error: Your Hub session token is invalid. Please run 'atuin login' to \
             re-authenticate with Atuin Hub."
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    #[rstest]
    #[case::wrong_password_change(
        AccountChange::Password,
        401,
        json!({"reason": "password is not correct"}),
        "error: current password is incorrect"
    )]
    #[case::bodyless_200(AccountChange::Password, 200, Value::Null, "changed")]
    #[case::other_success(AccountChange::Deletion, 204, Value::Null, "error: unknown error")]
    #[tokio::test]
    async fn legacy_account_changes_succeed_only_on_200(
        #[case] account_change: AccountChange,
        #[case] status: u16,
        #[case] body: Value,
        #[case] expected: &str,
    ) {
        let (verb, route, request) = legacy_request(account_change);
        let server = MockServer::start().await;
        Mock::given(method(verb))
            .and(path(route))
            .and(header("authorization", "Token sess"))
            .and(header("x-extra", "extra"))
            .and(body_json(request))
            .respond_with(answer(status, body, Some(ATUIN_CARGO_VERSION)))
            .expect(1)
            .mount(&server)
            .await;

        let response = change(&legacy(&server, Some("sess")), account_change).await;

        assert_eq!(changed(response), expected);
    }

    #[rstest]
    #[case::taken(200, json!({"username": "ellie"}), 0, "error: username already in use")]
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

    /// Logging in to a legacy server sends the user's `extra_headers` and Atuin's identity.
    #[rstest]
    #[tokio::test]
    async fn legacy_sign_in_sends_the_user_and_identity_headers() {
        let server = legacy_server(Some(ATUIN_CARGO_VERSION)).await;

        legacy(&server, None).login("ellie", &"pw".into(), None).await.unwrap();

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
                )
            })
            .collect();
        assert_eq!(sent, [(
            "/login",
            Some("extra"),
            Some(ATUIN_USER_AGENT),
            Some(ATUIN_CARGO_VERSION)
        )]);
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

    /// A username the lookup's path cannot carry is never looked up: `.` and `..` would resolve
    /// away, so the client refuses them, and a `\` would read as `/`, so the server's own refusal
    /// answers it.
    #[rstest]
    #[case::dot(".", "error: path segments cannot be . or ..", &[])]
    #[case::dot_dot("..", "error: path segments cannot be . or ..", &[])]
    #[case::backslash(
        "..\\",
        "/register, 400 Bad Request - Only alphanumeric and hyphens (-) are allowed in usernames.",
        &["/register"]
    )]
    #[tokio::test]
    async fn legacy_register_never_looks_up_a_path_like_username(
        #[case] username: &str,
        #[case] error_ending: &str,
        #[case] sent: &[&str],
    ) {
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

        let response =
            legacy(&server, None).register(username, "e@example.com", &"pw".into()).await;

        let response = signed_in(response);
        assert!(response.ends_with(error_ending), "{response}");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(), sent);
    }
}
