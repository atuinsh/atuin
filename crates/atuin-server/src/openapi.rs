//! OpenAPI description of the sync server API, derived from the handlers and
//! from the `atuin-common` wire types.
//!
//! This is the source of truth for `crates/atuin-client/openapi.json`, which in
//! turn is the input to the progenitor-generated `atuin-api-client`. Regenerate
//! the committed spec with:
//!
//! ```sh
//! cargo run -p atuin-server --bin atuin-server -- openapi > crates/atuin-client/openapi.json
//! ```
//!
//! `tests/openapi.rs` fails if the committed file drifts from this module, and
//! if a route in `router.rs` is missing from `paths(...)` below.
//!
//! NOTE: utoipa 4.x emits OpenAPI 3.0.3. progenitor only parses 3.0.x, so this
//! crate must not be bumped to utoipa 5.x (which emits 3.1.0 only) without
//! adding a 3.1 -> 3.0 downconvert step.

use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
};

use crate::handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Atuin Sync Server API",
        version = "0.1.0",
        description = "HTTP API for the atuin sync server (crates/atuin-server). Schemas are derived from the Rust types in atuin-common, so they mirror their serde output exactly. All error responses use the ErrorResponse shape {\"reason\": string}. Every response carries an `Atuin-Version` header with the server's cargo version; the client uses it to refuse syncing across major versions. If the server is configured with a URL path prefix, it must be included in the client base URL.",
    ),
    servers((url = "/")),
    paths(
        handlers::index,
        handlers::health::health_check,
        handlers::user::get,
        handlers::user::register,
        handlers::user::login,
        handlers::user::delete,
        handlers::user::change_password,
        handlers::v0::me::get,
        handlers::v0::record::post,
        handlers::v0::record::index,
        handlers::v0::record::next,
        handlers::v0::store::delete,
    ),
    components(schemas(
        atuin_common::api::IndexResponse,
        handlers::health::HealthResponse,
        atuin_common::api::UserResponse,
        atuin_common::api::RegisterRequest,
        atuin_common::api::RegisterResponse,
        atuin_common::api::LoginRequest,
        atuin_common::api::LoginResponse,
        atuin_common::api::DeleteUserResponse,
        atuin_common::api::ChangePasswordRequest,
        atuin_common::api::ChangePasswordResponse,
        atuin_common::api::MeResponse,
        atuin_common::api::ErrorResponse,
        atuin_common::record::EncryptedData,
        atuin_common::record::Host,
        atuin_common::record::RecordEncrypted,
        atuin_common::record::RecordStatus,
    )),
    modifiers(&SessionAuth),
)]
pub struct ApiDoc;

struct SessionAuth;

impl Modify for SessionAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .as_mut()
            .expect("components are always generated");

        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "Authorization",
                "Session token presented as `Authorization: Token <session-token>`. This server accepts only the `Token` scheme (exactly one space separator); any other scheme is rejected with 400 {\"reason\":\"invalid authorization header encoding\"}. The full header value including the `Token ` prefix must be supplied.",
            ))),
        );
    }
}

/// The OpenAPI document as pretty-printed JSON, matching the committed
/// `crates/atuin-client/openapi.json` byte for byte.
pub fn spec() -> String {
    let mut json = ApiDoc::openapi()
        .to_pretty_json()
        .expect("failed to serialize openapi document");
    json.push('\n');
    json
}
