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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use atuin_api_client::{ApiError, MapApiError};
    use rstest::rstest;
    use secrecy::ExposeSecret;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// No sync header (`extra_headers`, `Atuin-Version`, capabilities) reaches `ai.endpoint`.
    #[rstest]
    #[case::anonymous(None, &["accept", "accept-encoding", "host", "user-agent"])]
    #[case::authenticated(
        Some("atapi_token"),
        &["accept", "accept-encoding", "authorization", "host", "user-agent"]
    )]
    #[tokio::test]
    async fn sends_the_user_agent_and_the_token_only(
        #[case] token: Option<&str>,
        #[case] headers: &[&str],
    ) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/cli/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"default": "fast", "models": []})),
            )
            .mount(&server)
            .await;
        let endpoint = Url::parse(&server.uri()).unwrap();
        let token = token.map(SecretString::from);

        client(&endpoint, token.as_ref()).unwrap().list_models().map_api_error().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let [request] = requests.as_slice() else {
            panic!("expected one request, got {requests:?}");
        };
        let names: BTreeSet<_> = request.headers.keys().map(|name| name.as_str()).collect();
        assert_eq!(names, headers.iter().copied().collect());
        assert_eq!(request.headers[USER_AGENT], ATUIN_USER_AGENT);
        let bearer = token.map(|token| format!("Bearer {}", token.expose_secret()));
        assert_eq!(
            request.headers.get(AUTHORIZATION).map(|value| value.to_str().unwrap()),
            bearer.as_deref()
        );
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn gives_up_after_ten_seconds() {
        // Accepted by the kernel's backlog and never answered.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("http://{}/", silent.local_addr().unwrap())).unwrap();
        let started = tokio::time::Instant::now();

        let client = client(&endpoint, None).unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(60), client.list_models().map_api_error())
                .await
                .expect("the call must time out on its own");

        let Err(ApiError::Transport(err)) = result else {
            panic!("expected a transport error, got {result:?}");
        };
        assert!(err.is_timeout(), "{err:?}");
        assert_eq!(started.elapsed().as_secs(), 10);
    }
}
