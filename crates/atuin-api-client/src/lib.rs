//! Typed client for the Atuin sync, hub and AI HTTP APIs.

use std::collections::HashMap;
use std::time::Duration;

use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_USER_AGENT};
use derive_more::{Deref, From, Into};
use reqwest::header::{
    AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, InvalidHeaderName, InvalidHeaderValue,
    USER_AGENT,
};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

mod date_time;
mod error;
mod generated {
    #![allow(
        clippy::disallowed_methods,
        clippy::missing_errors_doc,
        clippy::missing_panics_doc,
        clippy::must_use_candidate,
        clippy::struct_field_names,
        clippy::unnecessary_trailing_comma,
        reason = "generated from openapi.json by progenitor"
    )]
    progenitor::generate_api!(
        spec = { path = "openapi.json", relative_to = OutDir },
        interface = Positional,
        inner_type = crate::HookState,
        // The record types carry invariants and the frozen field names of the PASETO implicit
        // assertion, so the client uses atuin-domain's rather than generating look-alikes.
        replace = {
            EncryptedData = ::atuin_domain::record::EncryptedData: ?FromStr + ?Display,
            Host = ::atuin_domain::record::Host: ?FromStr + ?Display,
            Record = ::atuin_domain::record::Record<::atuin_domain::record::EncryptedData>: ?FromStr + ?Display,
            RecordStatus = ::atuin_domain::record::RecordStatus: ?FromStr + ?Display,
        },
        convert = {
            { type = "string", format = "password" } = crate::Secret: ?FromStr + ?Display,
            { type = "integer", format = "int64", minimum = 0 } = u64: Default,
            { type = "string", format = "date-time" } = crate::DateTime: ?FromStr + ?Display,
            { type = "string", format = "uri" } = ::url::Url,
        },
        patch = {
            ModelInfo = { derives = [PartialEq, Eq] },
            ModelList = { derives = [PartialEq, Eq] },
            UsageBucket = { derives = [PartialEq, Eq] },
            UsageSnapshot = { derives = [PartialEq, Eq] },
        },
    );
}
mod hooks;

pub use date_time::DateTime;
pub use error::{ApiBody, ApiError, MapApiError};
pub use generated::{Client, ResponseValue, types};
pub use hooks::{AuthHeaderFuture, AuthHeaderProvider, HookState};

/// A secret the API carries in plaintext; its `Debug` stays redacted.
#[derive(Clone, Debug, Deserialize, Serialize, From, Into, Deref)]
#[from(SecretString, String, &str)]
#[serde(transparent)]
pub struct Secret(#[serde(serialize_with = "expose")] SecretString);

fn expose<S: Serializer>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

/// Why a [`Client`] could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("the token cannot be sent as an HTTP header")]
    Token(#[from] InvalidHeaderValue),
    #[error("invalid extra_headers name {name:?}")]
    ExtraHeaderName {
        name: String,
        #[source]
        source: InvalidHeaderName,
    },
    #[error("invalid extra_headers value for {name:?}")]
    ExtraHeaderValue {
        name: String,
        #[source]
        source: InvalidHeaderValue,
    },
    #[error("failed to build the HTTP client")]
    Http(#[from] reqwest::Error),
    #[error("a base URL needs a hierarchical path, like https://host/prefix")]
    CannotBeABase,
}

/// Authentication token for sync API requests.
///
/// The sync API supports two authentication methods:
/// - `Bearer`: Hub API tokens (for users authenticated via Atuin Hub)
/// - `Token`: Legacy CLI session tokens (for users registered via CLI or self-hosted)
///
/// When both are available, Hub tokens are preferred as they provide unified
/// authentication across CLI and Hub features.
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// Hub API token, used with "Bearer {token}" header
    Bearer(SecretString),
    /// Legacy CLI session token, used with "Token {token}" header
    Token(SecretString),
}

impl AuthToken {
    /// The token as a sensitive `Authorization` value, e.g. `Bearer atapi_...`.
    pub fn to_header_value(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        let (scheme, token) = match self {
            Self::Bearer(token) => ("Bearer", token),
            Self::Token(token) => ("Token", token),
        };
        let value = Zeroizing::new(format!("{scheme} {}", token.expose_secret()));
        let mut header = HeaderValue::from_str(&value)?;
        header.set_sensitive(true);
        Ok(header)
    }
}

/// How long a client waits to connect, and for a whole call.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub total: Duration,
}

impl Client {
    /// Build a client for the server at `base` that sends every request through `http`.
    pub fn from_http(mut base: Url, http: reqwest::Client) -> Result<Self, ClientBuildError> {
        if base.cannot_be_a_base() {
            return Err(ClientBuildError::CannotBeABase);
        }
        // The generated code formats `{prefix}/api/v0/me` as a string, so a trailing slash would
        // double and a query would land before the path; the query goes back on in the pre hook.
        let query = base.query().filter(|query| !query.is_empty()).map(str::to_owned);
        base.set_query(None);
        base.set_fragment(None);

        let prefix = base.as_str().trim_end_matches('/');

        #[allow(clippy::disallowed_methods, reason = "the one place that normalises the base")]
        Ok(Self::new_with_client(prefix, http, HookState::for_base(query)))
    }

    /// A client for `base` that authenticates every request with `auth`.
    ///
    /// `extra_headers` belong to the user's sync server, so a client for the hub or an AI server
    /// passes `None`.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when `auth` or an extra header cannot be a header, TLS cannot be set
    /// up, or `base` cannot be a base URL.
    pub fn connect_authenticated(
        base: &Url,
        auth: &AuthToken,
        timeouts: Timeouts,
        extra_headers: Option<&HashMap<String, SecretString>>,
    ) -> Result<Self, ClientBuildError> {
        Self::connect(base, Some(auth), timeouts, extra_headers)
    }

    /// A client for `base` that sends no credentials.
    ///
    /// `extra_headers` belong to the user's sync server, so a client for the hub or an AI server
    /// passes `None`.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when an extra header cannot be a header, TLS cannot be set up, or
    /// `base` cannot be a base URL.
    pub fn connect_unauthenticated(
        base: &Url,
        timeouts: Timeouts,
        extra_headers: Option<&HashMap<String, SecretString>>,
    ) -> Result<Self, ClientBuildError> {
        Self::connect(base, None, timeouts, extra_headers)
    }

    /// A client for `base` that sends Atuin's identity over `extra_headers`, and `auth`, if any,
    /// over both.
    fn connect(
        base: &Url,
        auth: Option<&AuthToken>,
        timeouts: Timeouts,
        extra_headers: Option<&HashMap<String, SecretString>>,
    ) -> Result<Self, ClientBuildError> {
        let mut headers = extra_headers.map(extra_headers_map).transpose()?.unwrap_or_default();
        headers.extend(identity_headers());
        if let Some(auth) = auth {
            headers.insert(AUTHORIZATION, auth.to_header_value()?);
        }

        let http = client_builder(extra_headers)
            .default_headers(headers)
            .connect_timeout(timeouts.connect)
            .timeout(timeouts.total)
            .build()?;
        Self::from_http(base.clone(), http)
    }
}

/// Atuin's `User-Agent` and `Atuin-Version`, which every call to an Atuin server carries.
fn identity_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(ATUIN_USER_AGENT));
    headers.insert(ATUIN_HEADER_VERSION, HeaderValue::from_static(ATUIN_CARGO_VERSION));
    headers
}

/// A [`reqwest::ClientBuilder`] appropriate for the given extra headers.
///
/// reqwest only strips its own well-known sensitive headers (Authorization,
/// Cookie, ...) when following a cross-host redirect; user-configured extra
/// headers would be forwarded as-is. Since those often carry credentials
/// (e.g. Cloudflare Access secrets), refuse cross-origin redirects entirely
/// whenever extra headers are configured.
fn client_builder(extra_headers: Option<&HashMap<String, SecretString>>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();

    if extra_headers.is_none_or(HashMap::is_empty) {
        return builder;
    }

    builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt.previous().last().is_some_and(|prev| {
            prev.scheme() == attempt.url().scheme()
                && prev.host_str() == attempt.url().host_str()
                && prev.port_or_known_default() == attempt.url().port_or_known_default()
        });

        if !same_origin {
            attempt.error(
                "refusing to follow cross-origin redirect: extra_headers are configured and will \
                 not be sent to a different origin",
            )
        } else if attempt.previous().len() > 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    }))
}

/// Build a [`HeaderMap`] from user-configured extra headers (the
/// `extra_headers` setting). Headers Atuin sets itself should be inserted
/// after these so that Atuin's values win.
///
/// Every value is marked sensitive, since these often carry credentials.
fn extra_headers_map(
    extra_headers: &HashMap<String, SecretString>,
) -> Result<HeaderMap, ClientBuildError> {
    let mut headers = HeaderMap::new();
    for (name, value) in extra_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|source| {
            ClientBuildError::ExtraHeaderName {
                name: name.clone(),
                source,
            }
        })?;
        let mut header_value = HeaderValue::from_str(value.expose_secret()).map_err(|source| {
            ClientBuildError::ExtraHeaderValue {
                name: name.clone(),
                source,
            }
        })?;
        header_value.set_sensitive(true);
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}
