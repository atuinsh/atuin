use semver::Version;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

// the usage of X- has been deprecated for quite along time, it turns out
pub static ATUIN_HEADER_VERSION: &str = "Atuin-Version";
pub static ATUIN_CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");

pub static ATUIN_VERSION: LazyLock<Version> =
    LazyLock::new(|| Version::parse(ATUIN_CARGO_VERSION).expect("failed to parse self semver"));

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub username: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session: String,
    /// Auth type: "hub" for Hub API tokens, "cli" for legacy CLI session tokens.
    /// Old servers that don't return this field will deserialize as None.
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteUserResponse {}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChangePasswordResponse {}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub session: String,
    /// Auth type: "hub" for Hub API tokens, "cli" for legacy CLI session tokens.
    /// Old servers that don't return this field will deserialize as None.
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse<'a> {
    pub reason: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexResponse {
    pub homage: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeResponse {
    pub username: String,
}

// Hub CLI authentication types

/// Response from `POST /auth/cli/code` - generates a code for CLI auth
#[derive(Debug, Serialize, Deserialize)]
pub struct CliCodeResponse {
    pub code: String,
}

/// Response from `GET /auth/cli/verify?code=<code>` - polls for authorization
#[derive(Debug, Serialize, Deserialize)]
pub struct CliVerifyResponse {
    /// Session token, present only when authorization is complete
    pub token: Option<String>,
    pub success: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    /// An opaque capability token issued by the server.
    ///
    /// The server is free to choose its own scheme (a hash, a monotonic counter, a UUID, ...); the
    /// client stores and echoes this value back verbatim in `X-Atuin-Capabilities-Known` and never
    /// interprets it. If the client and server disagree on this token, none of the capabilities are
    /// to be assumed.
    pub version: String,

    /// The list of capabilities this server supports. These will be provided as a JSON:
    ///
    /// ```json
    /// {
    ///   "sh.atuin.server/version": {
    ///     "version": 1,
    ///     "server_version": "x.y.z"
    ///   },
    ///   "sh.atuin.server/records.batch": {
    ///     "version": 1,
    ///     "max_batch_size": 128,
    ///     "min_batch_size": 12,
    ///     "max_bytes": 13
    ///   },
    ///   "sh.atuin.server/records.stdout": {
    ///     "version": 1
    ///   }
    /// }
    /// ```
    pub capabilities: HashMap<String, serde_json::Value>,
}
