//! Client-side reqwest middleware for HTTP capability negotiation.
//!
//! The middleware stamps the client's last-known capability token onto each request and, when the
//! server rejects with `412` plus a differing available token, refreshes capabilities (and, if so
//! configured, retries the request once).

use async_trait::async_trait;
use http::Extensions;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Request, Response, StatusCode};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware, Next, Result};
use typed_builder::TypedBuilder;

use crate::caps::CapClient;
use crate::caps::http::{AVAILABLE_HEADER, KNOWN_HEADER};

/// Reqwest middleware that negotiates capability versions with the server.
///
/// Stamps [`CapClient::known_token`] onto each request as `X-Atuin-Capabilities-Known`. When the
/// server answers `412` with a differing `X-Atuin-Capabilities-Available`, it refreshes
/// capabilities over its own plain [`reqwest::Client`] (concurrent refreshes coalesce) and, when
/// built with `refresh(true)`, retries the original request once with the fresh token.
#[derive(Debug, Clone, TypedBuilder)]
pub struct CapMiddleware {
    /// Source of the known token and the `/api/v0/capabilities` refresh.
    caps: CapClient,
    /// Client used to request the capabilities from the server.
    http: Client,
    /// Whether to retry the original request once after a successful refresh.
    #[builder(default = true)]
    refresh: bool,
}

#[async_trait]
impl Middleware for CapMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        ext: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        // Stamp our known token onto the request; a `None` or non-ASCII token leaves the header off
        // rather than panicking the request path.
        let known = self.caps.known_token();
        if let Some(value) = known.as_deref().and_then(|t| HeaderValue::from_str(t).ok()) {
            req.headers_mut()
                .insert(HeaderName::from_static(KNOWN_HEADER), value);
        }

        let retry_req = if self.refresh { req.try_clone() } else { None };

        let response = next.clone().run(req, ext).await?;

        let is_mismatch = response.status() == StatusCode::PRECONDITION_FAILED
            && response
                .headers()
                .get(AVAILABLE_HEADER)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|available| Some(available) != known.as_deref());
        if !is_mismatch {
            return Ok(response);
        }

        // Mismatch, but we cannot (or were told not to) retry -> surface the 412 untouched.
        let Some(mut retry_req) = retry_req else {
            return Ok(response);
        };

        // Refresh (coalesced across concurrent callers) and retry exactly once with the new token.
        self.caps.refresh(&self.http).await?;
        let fresh = self.caps.known_token();
        if let Some(value) = fresh.as_deref().and_then(|t| HeaderValue::from_str(t).ok()) {
            retry_req
                .headers_mut()
                .insert(HeaderName::from_static(KNOWN_HEADER), value);
        }

        next.run(retry_req, ext).await
    }
}

/// Install capability negotiation onto a [`reqwest::Client`].
pub trait CapabilitiesExt {
    /// Wrap this client so it negotiates capabilities.
    fn with_capabilities(self, caps: CapClient, refresh: bool) -> ClientWithMiddleware;
}

impl CapabilitiesExt for Client {
    fn with_capabilities(self, caps: CapClient, refresh: bool) -> ClientWithMiddleware {
        let middleware = CapMiddleware::builder()
            .caps(caps)
            .http(self.clone())
            .refresh(refresh)
            .build();
        ClientBuilder::new(self).with(middleware).build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapClient;
    use rstest::rstest;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Mount a caps endpoint (returns version 5) plus a `/protected` route that 200s only when the
    /// client presents `x-atuin-capabilities-known: 5`, and otherwise 412s with the available token.
    async fn negotiating_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "5",
                "capabilities": {}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("x-atuin-capabilities-known", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(
                ResponseTemplate::new(412).append_header("x-atuin-capabilities-available", "5"),
            )
            .with_priority(5)
            .mount(&server)
            .await;
        server
    }

    fn cap_client(server: &MockServer) -> CapClient {
        let caps_url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        CapClient::new(caps_url)
    }

    #[rstest]
    #[case(true, 200, 1)]
    #[case(false, 412, 0)]
    #[tokio::test]
    async fn refresh_controls_whether_the_412_is_retried(
        #[case] refresh: bool,
        #[case] expected_status: u16,
        #[case] expected_caps_hits: usize,
    ) {
        crate::tls::ensure_crypto_provider();
        let server = negotiating_server().await;
        let http = reqwest::Client::new();
        let middleware = CapMiddleware::builder()
            .caps(cap_client(&server))
            .http(http.clone())
            .refresh(refresh)
            .build();
        let client = ClientBuilder::new(http).with(middleware).build();

        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), expected_status);

        let caps_hits = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count();
        assert_eq!(caps_hits, expected_caps_hits);
    }

    #[tokio::test]
    async fn unrelated_4xx_is_not_touched() {
        crate::tls::ensure_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let caps_url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();

        let http = reqwest::Client::new();
        let middleware = CapMiddleware::builder()
            .caps(CapClient::new(caps_url))
            .http(http.clone())
            .refresh(true)
            .build();
        let client = ClientBuilder::new(http).with(middleware).build();

        let response = client
            .get(format!("{}/missing", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
        let caps_hits = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count();
        assert_eq!(caps_hits, 0, "a plain 404 must not trigger a refresh");
    }

    #[tokio::test]
    async fn bare_412_without_caps_header_passes_through() {
        crate::tls::ensure_crypto_provider();
        // A 412 that carries no `X-Atuin-Capabilities-Available` header is an ordinary precondition
        // failure, not a capability signal -- the middleware must pass it through and never refresh.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/precondition"))
            .respond_with(ResponseTemplate::new(412))
            .mount(&server)
            .await;
        let caps_url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();

        let http = reqwest::Client::new();
        let middleware = CapMiddleware::builder()
            .caps(CapClient::new(caps_url))
            .http(http.clone())
            .refresh(true)
            .build();
        let client = ClientBuilder::new(http).with(middleware).build();

        let response = client
            .get(format!("{}/precondition", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 412);
        let caps_hits = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count();
        assert_eq!(
            caps_hits, 0,
            "a 412 without the caps header must not trigger a refresh"
        );
    }

    #[tokio::test]
    async fn ext_trait_builds_a_negotiating_client() {
        crate::tls::ensure_crypto_provider();
        let server = negotiating_server().await;
        let client = reqwest::Client::new().with_capabilities(cap_client(&server), true);

        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn concurrent_burst_fetches_capabilities_once() {
        crate::tls::ensure_crypto_provider();
        let server = negotiating_server().await;
        let client = reqwest::Client::new().with_capabilities(cap_client(&server), true);

        let mut handles = Vec::new();
        for _ in 0..20 {
            let client = client.clone();
            let url = format!("{}/protected", server.uri());
            handles.push(tokio::spawn(async move {
                client.get(url).send().await.unwrap().status()
            }));
        }
        for handle in handles {
            assert_eq!(handle.await.unwrap(), 200);
        }

        let caps_hits = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count();
        assert_eq!(
            caps_hits, 1,
            "a burst must coalesce into one capabilities fetch"
        );
    }
}
