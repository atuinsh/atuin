use semver::Version;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::record::RecordId;
use url::Url;

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

/// The capabilities a server advertises, as returned from its capabilities endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    /// An opaque capability token issued by the server.
    pub version: String,

    /// The list of capabilities this server supports, as a map of capability name to its value.
    pub capabilities: HashMap<String, serde_json::Value>,
}

/// Response to `POST /api/v0/bundles`: where to upload the packfile body, and the id the
/// server assigned the bundle.
#[derive(Debug, Serialize, Deserialize)]
pub struct BundleResponse {
    pub upload_url: Url,
    pub bundle_id: RecordId,
}

/// Response to `GET /api/v0/bundles/{manifest_id}`: where to download the packfile body.
#[derive(Debug, Serialize, Deserialize)]
pub struct BundleDownloadResponse {
    pub download_url: Url,
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
