//! Headers and clients shared by every call to an Atuin server.

use atuin_api_client::authorization;
use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_USER_AGENT};
use eyre::Result;
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use secrecy::SecretString;

/// Atuin's `User-Agent` and `Atuin-Version`, which every call to an Atuin server carries.
pub fn identity_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(ATUIN_USER_AGENT));
    headers.insert(ATUIN_HEADER_VERSION, HeaderValue::from_static(ATUIN_CARGO_VERSION));
    headers
}

/// A client for the hub at `endpoint`, authenticated with the hub API token `bearer`, if any.
///
/// Sets no timeouts and sends none of the user's `extra_headers`.
pub fn hub_client(
    endpoint: &Url,
    bearer: Option<&SecretString>,
) -> Result<atuin_api_client::Client> {
    let mut headers = identity_headers();
    if let Some(token) = bearer {
        headers.insert(AUTHORIZATION, authorization("Bearer", token)?);
    }
    let http = reqwest::Client::builder().default_headers(headers).build()?;
    Ok(atuin_api_client::Client::from_http(endpoint, http)?)
}
