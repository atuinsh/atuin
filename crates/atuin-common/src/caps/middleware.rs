//! Client-side reqwest middleware for HTTP capability negotiation.
//!
//! The middleware stamps the client's last-known capability token onto each request and, when the
//! server rejects with `412` plus a differing available token, refreshes capabilities (and, if so
//! configured, retries the request once).

use async_trait::async_trait;
use http::Extensions;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Request, Response, StatusCode};
use reqwest_middleware::{Middleware, Next, Result};
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
    return (Some(available) != known).then(|| available.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapClient;
    use reqwest_middleware::ClientBuilder;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Mount a caps endpoint (returns version 5) plus a `/protected` route that 200s only when the
    /// client presents `x-atuin-capabilities-known: 5`, and otherwise 412s with the available token.
    async fn negotiating_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": 5,
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
        let caps_url = format!("{}/api/v0/capabilities", server.uri()).parse().unwrap();
        CapClient::new(caps_url)
    }

    #[tokio::test]
    async fn refresh_true_retries_transparently() {
        crate::tls::ensure_crypto_provider();
        let server = negotiating_server().await;
        let http = reqwest::Client::new();
        let middleware = CapMiddleware::builder()
            .caps(cap_client(&server))
            .http(http.clone())
            .refresh(true)
            .build();
        let client = ClientBuilder::new(http).with(middleware).build();

        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "ok");

        let caps_hits = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count();
        assert_eq!(caps_hits, 1, "capabilities should be fetched exactly once");
    }

    #[tokio::test]
    async fn refresh_false_surfaces_the_412() {
        crate::tls::ensure_crypto_provider();
        let server = negotiating_server().await;
        let http = reqwest::Client::new();
        let middleware = CapMiddleware::builder()
            .caps(cap_client(&server))
            .http(http.clone())
            .refresh(false)
            .build();
        let client = ClientBuilder::new(http).with(middleware).build();

        let response = client
            .get(format!("{}/protected", server.uri()))
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
        assert_eq!(caps_hits, 0, "refresh(false) must not fetch capabilities");
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
        let caps_url = format!("{}/api/v0/capabilities", server.uri()).parse().unwrap();

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

    #[test]
    fn mismatch_when_412_and_available_differs() {
        let response = response_with(412, Some("5"));
        assert_eq!(capability_mismatch(&response, Some("3")), Some("5".to_string()));
    }

    #[test]
    fn mismatch_when_412_and_client_knows_nothing() {
        let response = response_with(412, Some("5"));
        assert_eq!(capability_mismatch(&response, None), Some("5".to_string()));
    }

    #[test]
    fn no_mismatch_when_tokens_match() {
        let response = response_with(412, Some("5"));
        assert_eq!(capability_mismatch(&response, Some("5")), None);
    }

    #[test]
    fn no_mismatch_when_status_is_not_412() {
        let response = response_with(200, Some("5"));
        assert_eq!(capability_mismatch(&response, Some("3")), None);
    }

    #[test]
    fn no_mismatch_when_412_lacks_the_available_header() {
        let response = response_with(412, None);
        assert_eq!(capability_mismatch(&response, Some("3")), None);
    }

    #[test]
    fn unrelated_4xx_passes_through() {
        let response = response_with(404, None);
        assert_eq!(capability_mismatch(&response, Some("3")), None);
    }

    #[test]
    fn stamp_known_sets_the_header_when_a_token_is_present() {
        crate::tls::ensure_crypto_provider();
        let mut req = reqwest::Client::new()
            .get("http://example.invalid/x")
            .build()
            .unwrap();
        stamp_known(&mut req, Some("9"));
        assert_eq!(
            req.headers().get(KNOWN_HEADER).unwrap().to_str().unwrap(),
            "9"
        );
    }

    #[test]
    fn stamp_known_leaves_the_header_absent_when_token_is_none() {
        crate::tls::ensure_crypto_provider();
        let mut req = reqwest::Client::new()
            .get("http://example.invalid/x")
            .build()
            .unwrap();
        stamp_known(&mut req, None);
        assert!(req.headers().get(KNOWN_HEADER).is_none());
    }
}
