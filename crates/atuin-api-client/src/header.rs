use std::collections::HashMap;
use std::time::Duration;

use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_USER_AGENT};
use reqwest::header::{
    AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, InvalidHeaderName, InvalidHeaderValue,
    USER_AGENT,
};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::Client;

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
    /// Format the token as a sensitive Authorization header value.
    ///
    /// # Errors
    ///
    /// [`InvalidHeaderValue`] when the token holds a byte a header cannot carry, e.g. a newline.
    pub fn to_header_value(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        match self {
            Self::Bearer(token) => authorization("Bearer", token),
            Self::Token(token) => authorization("Token", token),
        }
    }
}

/// Atuin's `User-Agent` and `Atuin-Version`, which every call to an Atuin server carries.
#[must_use]
fn identity_headers() -> HeaderMap {
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
        Self::from_http(endpoint, http)
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
        Self::from_http(endpoint, http)
    }

    /// A sync client for `base` that sends `auth` and Atuin's identity over `extra_headers`.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when `auth` or an extra header cannot be a header, TLS cannot be set
    /// up, or `base` cannot be a base URL.
    pub fn for_sync(
        base: &Url,
        auth: &AuthToken,
        extra_headers: &HashMap<String, SecretString>,
        connect_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self, ClientBuildError> {
        let mut headers = extra_headers_map(extra_headers)?;
        headers.extend(identity_headers());
        headers.insert(AUTHORIZATION, auth.to_header_value()?);

        let http = client_builder(extra_headers)
            .default_headers(headers)
            .connect_timeout(connect_timeout)
            .timeout(timeout)
            .build()?;
        Self::from_http(base, http)
    }

    /// Equivalent to [`Self::for_sync`], but with no credentials of its own and no timeouts.
    ///
    /// # Errors
    ///
    /// [`ClientBuildError`] when an extra header cannot be a header, TLS cannot be set up, or
    /// `base` cannot be a base URL.
    pub fn for_sync_anonymous(
        base: &Url,
        extra_headers: &HashMap<String, SecretString>,
    ) -> Result<Self, ClientBuildError> {
        let mut headers = extra_headers_map(extra_headers)?;
        headers.extend(identity_headers());

        let http = client_builder(extra_headers).default_headers(headers).build()?;
        Self::from_http(base, http)
    }
}

/// A [`reqwest::ClientBuilder`] appropriate for the given extra headers.
///
/// reqwest only strips its own well-known sensitive headers (Authorization,
/// Cookie, ...) when following a cross-host redirect; user-configured extra
/// headers would be forwarded as-is. Since those often carry credentials
/// (e.g. Cloudflare Access secrets), refuse cross-origin redirects entirely
/// whenever extra headers are configured.
fn client_builder(extra_headers: &HashMap<String, SecretString>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();

    if extra_headers.is_empty() {
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

/// `<scheme> <token>` as a sensitive `Authorization` value, e.g. `Bearer atapi_...`.
///
/// Sensitive, so the token stays out of `Debug` output and HTTP/2 header compression.
///
/// # Errors
///
/// [`InvalidHeaderValue`] when the token holds a byte a header cannot carry, e.g. a newline.
fn authorization(
    scheme: &str,
    token: &SecretString,
) -> Result<HeaderValue, InvalidHeaderValue> {
    let value = Zeroizing::new(format!("{scheme} {}", token.expose_secret()));
    let mut header = HeaderValue::from_str(&value)?;
    header.set_sensitive(true);
    Ok(header)
}

#[cfg(test)]
mod tests {
    use rstest::*;

    use super::*;

    #[fixture]
    fn extra_headers() -> HashMap<String, SecretString> {
        let mut extra = HashMap::new();
        extra.insert("X-Auth-Token".to_string(), "secret".into());
        extra
    }

    #[rstest]
    fn extra_headers_map_parses_headers(extra_headers: HashMap<String, SecretString>) {
        let headers = extra_headers_map(&extra_headers).unwrap();
        let value = headers.get("x-auth-token").unwrap();
        assert_eq!(value, "secret");
        assert!(value.is_sensitive());
    }

    #[rstest]
    fn extra_headers_map_rejects_invalid_names() {
        let mut extra = HashMap::new();
        extra.insert("bad header".to_string(), "value".into());
        assert!(extra_headers_map(&extra).is_err());
    }

    /// Serve a single connection with a canned HTTP response.
    async fn serve_one(listener: &tokio::net::TcpListener, response: String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        sock.write_all(response.as_bytes()).await.unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn cross_origin_redirects_refused_with_extra_headers(
        extra_headers: HashMap<String, SecretString>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // A different port on the same host is a different origin
        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/\r\nContent-Length: \
                     0\r\nConnection: close\r\n\r\n",
                    port + 1
                ),
            )
            .await;
        });

        let client = client_builder(&extra_headers).build().unwrap();
        let err = client.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap_err();

        assert!(err.is_redirect(), "expected a redirect policy error: {err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn same_origin_redirects_followed_with_extra_headers(
        extra_headers: HashMap<String, SecretString>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: \
                     http://127.0.0.1:{port}/ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
            .await;
            serve_one(
                &listener,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            )
            .await;
        });

        let client = client_builder(&extra_headers).build().unwrap();
        let resp = client.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.url().path(), "/ok");
    }
}
