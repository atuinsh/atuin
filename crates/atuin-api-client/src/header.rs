use std::time::Duration;

use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_USER_AGENT};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, InvalidHeaderValue, USER_AGENT};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::{BaseUrlError, Client};

/// Why a [`Client`] could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("the token cannot be sent as an HTTP header")]
    Token(#[from] InvalidHeaderValue),
    #[error("failed to build the HTTP client")]
    Http(#[from] reqwest::Error),
    #[error("invalid base URL")]
    BaseUrl(#[from] BaseUrlError),
}

/// Atuin's `User-Agent` and `Atuin-Version`, which every call to an Atuin server carries.
#[must_use]
pub fn identity_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(ATUIN_USER_AGENT));
    headers.insert(ATUIN_HEADER_VERSION, HeaderValue::from_static(ATUIN_CARGO_VERSION));
    headers
}

impl Client {
    /// A client for the hub at `endpoint`, authenticated with the hub API token `bearer`, if any.
    ///
    /// Sets no timeouts and sends none of the user's `extra_headers`.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when the token cannot be a header, TLS cannot be set up, or `endpoint`
    /// cannot be a base URL.
    pub fn for_hub(endpoint: &Url, bearer: Option<&SecretString>) -> Result<Self, ClientBuildError> {
        let mut headers = identity_headers();
        if let Some(token) = bearer {
            headers.insert(AUTHORIZATION, authorization("Bearer", token)?);
        }
        let http = reqwest::Client::builder().default_headers(headers).build()?;
        Ok(Self::from_http(endpoint, http)?)
    }

    /// A client for the AI server at `endpoint` that sends `token`, if any, as a bearer.
    ///
    /// Sends `User-Agent` and nothing of the sync client's (no `extra_headers`, `Atuin-Version` or
    /// capability headers), since `ai.endpoint` can be a third-party origin.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when the token cannot be a header, TLS cannot be set up, or `endpoint`
    /// cannot be a base URL.
    pub fn for_ai(endpoint: &Url, token: Option<&SecretString>) -> Result<Self, ClientBuildError> {
        // Bounds a whole call, answer body included.
        const TIMEOUT: Duration = Duration::from_secs(10);

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(ATUIN_USER_AGENT));
        if let Some(token) = token {
            headers.insert(AUTHORIZATION, authorization("Bearer", token)?);
        }
        let http = reqwest::Client::builder().default_headers(headers).timeout(TIMEOUT).build()?;
        Ok(Self::from_http(endpoint, http)?)
    }
}

/// `<scheme> <token>` as a sensitive `Authorization` value, e.g. `Bearer atapi_...`.
///
/// Sensitive, so the token stays out of `Debug` output and HTTP/2 header compression.
///
/// # Errors
///
/// [`InvalidHeaderValue`] when the token holds a byte a header cannot carry, e.g. a newline.
pub fn authorization(
    scheme: &str,
    token: &SecretString,
) -> Result<HeaderValue, InvalidHeaderValue> {
    let value = Zeroizing::new(format!("{scheme} {}", token.expose_secret()));
    let mut header = HeaderValue::from_str(&value)?;
    header.set_sensitive(true);
    Ok(header)
}
