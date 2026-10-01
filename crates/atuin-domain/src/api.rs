use std::borrow::Cow;
use std::sync::LazyLock;

use secrecy::SecretString;
use semver::Version;
use serde::{Deserialize, Serialize};

// the usage of X- has been deprecated for quite along time, it turns out
pub static ATUIN_HEADER_VERSION: &str = "Atuin-Version";
pub static ATUIN_CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The `User-Agent` Atuin's clients send, e.g. `atuin/18.23.0`.
pub static ATUIN_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

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
    #[serde(serialize_with = "crate::secret::serialize")]
    pub password: SecretString,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    #[serde(serialize_with = "crate::secret::serialize")]
    pub session: SecretString,
    /// Auth type: "hub" for Hub API tokens, "cli" for legacy CLI session tokens.
    /// Old servers that don't return this field will deserialize as None.
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteUserResponse {}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    #[serde(serialize_with = "crate::secret::serialize")]
    pub current_password: SecretString,
    #[serde(serialize_with = "crate::secret::serialize")]
    pub new_password: SecretString,
    #[serde(
        default,
        serialize_with = "crate::secret::serialize_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub totp_code: Option<SecretString>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChangePasswordResponse {}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    #[serde(serialize_with = "crate::secret::serialize")]
    pub password: SecretString,
    #[serde(
        default,
        serialize_with = "crate::secret::serialize_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub totp_code: Option<SecretString>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(serialize_with = "crate::secret::serialize")]
    pub session: SecretString,
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
