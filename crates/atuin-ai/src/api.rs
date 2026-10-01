//! The generated client for the AI server's JSON routes; the chat stream is hand-written in
//! `stream`.
//!
//! A call sends `User-Agent: atuin/<version>` and, given a token, `Authorization: Bearer`, and
//! nothing of the sync client's: no `extra_headers`, `Atuin-Version` or capability headers, since
//! `ai.endpoint` can be a third-party origin.

use std::time::Duration;

use atuin_api_client::{BaseUrlError, Client, authorization};
use atuin_domain::api::ATUIN_USER_AGENT;
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, InvalidHeaderValue, USER_AGENT};
use secrecy::SecretString;

/// Bound on a whole call, answer body included.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Errors returned by [`client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Endpoint(#[from] BaseUrlError),
    #[error("the API token is not a valid HTTP header value")]
    Token(#[source] InvalidHeaderValue),
    #[error("failed to build the HTTP client")]
    Http(#[source] reqwest::Error),
}

/// A client for the AI server at `endpoint` that sends `token`, if any, as a bearer.
pub fn client(endpoint: &Url, token: Option<&SecretString>) -> Result<Client, ClientError> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(ATUIN_USER_AGENT));
    if let Some(token) = token {
        headers.insert(AUTHORIZATION, authorization("Bearer", token).map_err(ClientError::Token)?);
    }
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(TIMEOUT)
        .build()
        .map_err(ClientError::Http)?;
    Ok(Client::from_http(endpoint, http)?)
}
