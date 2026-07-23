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

/// Request header carrying the token the client believes is current.
const KNOWN_HEADER: &str = "x-atuin-capabilities-known";
/// Response header carrying the token the server actually has.
const AVAILABLE_HEADER: &str = "x-atuin-capabilities-available";

/// Stamp the known-capability token onto `req`, overwriting any prior value.
///
/// A `None` token (capabilities never fetched) leaves the request without the header, which the
/// server reads as "the client knows nothing". A token that is not a valid header value is skipped
/// rather than panicking -- the token is server-issued and expected to be ASCII, but we never trust
/// it enough to crash the request path.
fn stamp_known(req: &mut Request, token: Option<&str>) {
    let Some(token) = token else {
        return;
    };
    let Ok(value) = HeaderValue::from_str(token) else {
        return;
    };
    req.headers_mut()
        .insert(HeaderName::from_static(KNOWN_HEADER), value);
}

/// Decide whether `response` is a capability mismatch relative to the `known` token we sent.
///
/// Returns `Some(available_token)` only when all three hold: the status is `412 Precondition
/// Failed`, an `X-Atuin-Capabilities-Available` header is present, and its value differs from
/// `known`. Every other response -- a plain 4xx, a 412 without the header, a matching token --
/// yields `None` and is passed through untouched. The response body is never inspected.
fn capability_mismatch(response: &Response, known: Option<&str>) -> Option<String> {
    if response.status() != StatusCode::PRECONDITION_FAILED {
        return None;
    }
    let available = response.headers().get(AVAILABLE_HEADER)?.to_str().ok()?;
    (Some(available) != known).then(|| available.to_string())
}

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
    /// Plain client used only for the refresh fetch -- never the wrapped client, to avoid
    /// re-entering this middleware.
    http: Client,
    /// Whether to retry the original request once after a successful refresh. Defaults to `false`
    /// (surface the 412 to the caller).
    #[builder(default = false)]
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
        let known = self.caps.known_token();
        stamp_known(&mut req, known.as_deref());

        // Save a replayable copy only if we might retry. `try_clone` yields `None` for streaming
        // bodies, in which case we cannot retry and will surface the 412 instead.
        let retry_req = if self.refresh { req.try_clone() } else { None };

        let response = next.clone().run(req, ext).await?;

        // Not a capability mismatch -> pass the response straight through.
        if capability_mismatch(&response, known.as_deref()).is_none() {
            return Ok(response);
        }

        // Mismatch, but we cannot (or were told not to) retry -> surface the 412 untouched.
        let Some(mut retry_req) = retry_req else {
            return Ok(response);
        };

        // Refresh (coalesced across concurrent callers) and retry exactly once with the new token.
        self.caps.refresh(&self.http).await?;
        stamp_known(&mut retry_req, self.caps.known_token().as_deref());
        return next.run(retry_req, ext).await;
    }
}

/// Install capability negotiation onto a [`reqwest::Client`].
///
/// The caller supplies their configured client once; `with_capabilities` clones it for the
/// middleware's refresh path and wraps the original, so the wiring never leaks to the caller.
pub trait CapabilitiesExt {
    /// Wrap this client so it negotiates capabilities. `refresh` controls whether a stale-token
    /// rejection is transparently refreshed-and-retried (`true`) or surfaced to the caller
    /// (`false`).
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

    fn response_with(status: u16, available: Option<&str>) -> Response {
        let mut builder = http::Response::builder().status(status);
        if let Some(token) = available {
            builder = builder.header(AVAILABLE_HEADER, token);
        }
        Response::from(builder.body(String::new()).unwrap())
    }

    #[rstest]
    #[case(412, Some("5"), Some("3"), Some("5"))] // differs -> mismatch
    #[case(412, Some("5"), None, Some("5"))] // client knows nothing
    #[case(412, Some("5"), Some("5"), None)] // equal -> none
    #[case(200, Some("5"), Some("3"), None)] // not 412
    #[case(412, None, Some("3"), None)] // no available header
    #[case(404, None, Some("3"), None)] // unrelated 4xx
    fn capability_mismatch_cases(
        #[case] status: u16,
        #[case] available: Option<&str>,
        #[case] known: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let response = response_with(status, available);
        assert_eq!(
            capability_mismatch(&response, known),
            expected.map(String::from)
        );
    }

    #[rstest]
    #[case(Some("9"), Some("9"))]
    #[case(None, None)]
    fn stamp_known_cases(#[case] token: Option<&str>, #[case] expected: Option<&str>) {
        crate::tls::ensure_crypto_provider();
        let mut req = reqwest::Client::new()
            .get("http://example.invalid/x")
            .build()
            .unwrap();
        stamp_known(&mut req, token);
        match expected {
            Some(value) => {
                assert_eq!(
                    req.headers().get(KNOWN_HEADER).unwrap().to_str().unwrap(),
                    value
                );
            }
            None => assert!(req.headers().get(KNOWN_HEADER).is_none()),
        }
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
