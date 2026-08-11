//! Client-side reqwest middleware for HTTP capability negotiation.
//!
//! The middleware stamps the client's last-known capability token onto each request. When that
//! token is stale, [`CapMismatch`] decides what happens: `Continue` lets the server serve the
//! request and refreshes capabilities in the background; `Error` asks the server to reject with
//! `412`. The original request is never resent.

use std::sync::Arc;

use async_trait::async_trait;
use http::Extensions;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Request, Response};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware, Next, Result};
use typed_builder::TypedBuilder;

use crate::caps::CapClient;
use crate::caps::http::{AVAILABLE_HEADER, ENFORCE_HEADER, KNOWN_HEADER};

/// How the client reacts when its capability token is out of date with the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapMismatch {
    /// Let the server serve the request despite the mismatch, then refresh capabilities in the
    /// background so later requests are current. The original request is never resent.
    Continue,
    /// Ask the server (via `X-Atuin-Capabilities-Enforce`) to reject the request with `412` on a
    /// mismatch, surfacing it to the caller.
    Error,
}

/// Reqwest middleware that negotiates capability versions with the server.
///
/// Stamps `CapClient::known_token` onto each request as `X-Atuin-Capabilities-Known`. In
/// [`CapMismatch::Error`] mode it also sends `X-Atuin-Capabilities-Enforce`, so the server answers
/// `412` on a stale token. In [`CapMismatch::Continue`] mode (the default) the server serves the
/// request and returns `X-Atuin-Capabilities-Available`; the middleware then refreshes capabilities
/// over its own plain [`reqwest::Client`] in the background (concurrent refreshes coalesce) without
/// resending the request.
#[derive(Debug, Clone, TypedBuilder)]
pub struct CapMiddleware {
    /// Source of the known token and the `/api/v0/capabilities` refresh.
    caps: Arc<CapClient>,
    /// How to react to a capability-token mismatch with the server.
    #[builder(default = CapMismatch::Continue)]
    on_mismatch: CapMismatch,
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

        // In `Error` mode, ask the server to reject a stale token rather than serve the request.
        if self.on_mismatch == CapMismatch::Error {
            req.headers_mut().insert(
                HeaderName::from_static(ENFORCE_HEADER),
                HeaderValue::from_static("1"),
            );
        }

        let response = next.run(req, ext).await?;

        // If the server advertised a token differing from ours, our capabilities are stale.
        if self.on_mismatch == CapMismatch::Continue {
            let advertised = response
                .headers()
                .get(AVAILABLE_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|available| Some(*available) != known.as_deref())
                .map(str::to_owned);
            if let Some(available) = advertised
                && self.caps.known_token().as_deref() != Some(available.as_str())
            {
                // Coalesced and idempotent, so a burst drives exactly one fetch. Best-effort: a
                // refresh failure must not fail the request the server already served.
                let caps = self.caps.clone();
                tokio::spawn(async move {
                    let _ = caps.refresh_if_stale(&available).await;
                });
            }
        }

        Ok(response)
    }
}

/// Install capability negotiation onto a [`reqwest::Client`].
pub trait CapabilitiesExt {
    /// Wrap this client so it negotiates capabilities, reacting to a token mismatch per
    /// `on_mismatch`.
    fn with_capabilities(
        self,
        caps: Arc<CapClient>,
        on_mismatch: CapMismatch,
    ) -> ClientWithMiddleware;
}

impl CapabilitiesExt for Client {
    fn with_capabilities(
        self,
        caps: Arc<CapClient>,
        on_mismatch: CapMismatch,
    ) -> ClientWithMiddleware {
        let middleware = CapMiddleware::builder()
            .caps(caps)
            .on_mismatch(on_mismatch)
            .build();
        ClientBuilder::new(self).with(middleware).build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapClient;
    use rstest::{fixture, rstest};
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A plain reqwest client for the network tests.
    #[fixture]
    fn http_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    /// Mount a caps endpoint (version 5) plus a `/protected` route modelling the new server:
    /// a matching `x-atuin-capabilities-known: 5` -> `200`; a stale token *with*
    /// `x-atuin-capabilities-enforce` -> `412` + available token; a stale token *without* enforce
    /// -> `200` + available token (served anyway).
    #[fixture]
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
        // Current: the client's token matches.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("x-atuin-capabilities-known", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .with_priority(1)
            .mount(&server)
            .await;
        // Stale + enforce: reject.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header_exists("x-atuin-capabilities-enforce"))
            .respond_with(
                ResponseTemplate::new(412).append_header("x-atuin-capabilities-available", "5"),
            )
            .with_priority(2)
            .mount(&server)
            .await;
        // Stale, no enforce: serve anyway, but advertise our token.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("ok")
                    .append_header("x-atuin-capabilities-available", "5"),
            )
            .with_priority(5)
            .mount(&server)
            .await;
        server
    }

    async fn caps_hits(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v0/capabilities")
            .count()
    }

    /// Wait (bounded) for the background refresh to reach the capabilities endpoint. Coalescing is
    /// guaranteed by `refresh_if_stale`, so the count never exceeds one afterwards.
    async fn await_caps_hit(server: &MockServer) {
        for _ in 0..200 {
            if caps_hits(server).await > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    fn cap_client(server: &MockServer) -> Arc<CapClient> {
        let caps_url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();
        CapClient::new(caps_url, reqwest::Client::new())
    }

    #[rstest]
    #[tokio::test]
    async fn continue_serves_the_request_and_refreshes_in_the_background(
        http_client: reqwest::Client,
        #[future] negotiating_server: MockServer,
    ) {
        let server = negotiating_server.await;
        let client = http_client.with_capabilities(cap_client(&server), CapMismatch::Continue);

        // Our token is unknown -> stale, but the server serves the request anyway.
        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "ok");

        // The advertised token drives a single background refresh -- the request is never resent.
        await_caps_hit(&server).await;
        assert_eq!(caps_hits(&server).await, 1);
    }

    #[rstest]
    #[tokio::test]
    async fn error_surfaces_the_412_and_does_not_refresh(
        http_client: reqwest::Client,
        #[future] negotiating_server: MockServer,
    ) {
        let server = negotiating_server.await;
        let client = http_client.with_capabilities(cap_client(&server), CapMismatch::Error);

        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 412);

        // The only caps fetch is the eager warm-up on construction; `Error` mode adds no refresh.
        await_caps_hit(&server).await;
        assert_eq!(caps_hits(&server).await, 1);
    }

    #[rstest]
    #[tokio::test]
    async fn unrelated_4xx_is_not_touched(http_client: reqwest::Client) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let caps_url = format!("{}/api/v0/capabilities", server.uri())
            .parse()
            .unwrap();

        let middleware = CapMiddleware::builder()
            .caps(CapClient::new(caps_url, http_client.clone()))
            .on_mismatch(CapMismatch::Continue)
            .build();
        let client = ClientBuilder::new(http_client).with(middleware).build();

        let response = client
            .get(format!("{}/missing", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
        // The eager warm-up is the only caps fetch; a plain 404 adds no further refresh.
        await_caps_hit(&server).await;
        assert_eq!(
            caps_hits(&server).await,
            1,
            "a plain 404 must not trigger a refresh beyond the eager warm-up"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn bare_412_without_caps_header_passes_through(http_client: reqwest::Client) {
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

        let middleware = CapMiddleware::builder()
            .caps(CapClient::new(caps_url, http_client.clone()))
            .on_mismatch(CapMismatch::Continue)
            .build();
        let client = ClientBuilder::new(http_client).with(middleware).build();

        let response = client
            .get(format!("{}/precondition", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 412);
        // The eager warm-up is the only caps fetch; a 412 without the caps header adds no refresh.
        await_caps_hit(&server).await;
        assert_eq!(
            caps_hits(&server).await,
            1,
            "a 412 without the caps header must not trigger a refresh beyond the eager warm-up"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn ext_trait_builds_a_negotiating_client(
        http_client: reqwest::Client,
        #[future] negotiating_server: MockServer,
    ) {
        let server = negotiating_server.await;
        let client = http_client.with_capabilities(cap_client(&server), CapMismatch::Continue);

        let response = client
            .get(format!("{}/protected", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[rstest]
    #[tokio::test]
    async fn concurrent_burst_fetches_capabilities_once(
        http_client: reqwest::Client,
        #[future] negotiating_server: MockServer,
    ) {
        let server = negotiating_server.await;
        let client = http_client.with_capabilities(cap_client(&server), CapMismatch::Continue);

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

        // The background refreshes coalesce into a single capabilities fetch.
        await_caps_hit(&server).await;
        assert_eq!(
            caps_hits(&server).await,
            1,
            "a burst must coalesce into one capabilities fetch"
        );
    }
}
