//! Capability negotiation through the client hooks: a burst of answers advertising another token
//! refreshes the capabilities once.

use std::sync::Arc;
use std::time::Duration;

use atuin_api_client::{Client, MapApiError};
use atuin_domain::caps::{CapClient, CapMismatch, CapabilitiesCap};
use pretty_assertions::assert_eq;
use reqwest::StatusCode;
use rstest::{fixture, rstest};
use serde_json::json;
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CAPABILITIES: &str = "/api/v0/capabilities";
const ME: &str = "/api/v0/me";
const KNOWN: &str = "x-atuin-capabilities-known";
const AVAILABLE: &str = "x-atuin-capabilities-available";

fn capabilities(version: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"version": version, "capabilities": {}}))
}

/// A server whose capabilities move from version `4`, served to the first fetch, to `5`, plus a
/// negotiating `GET /api/v0/me`: a known token of `5` gets `200`; a stale token is served anyway,
/// with the available token.
#[fixture]
async fn negotiating_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CAPABILITIES))
        .respond_with(capabilities("4"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(CAPABILITIES))
        .respond_with(capabilities("5"))
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(ME))
        .and(header(KNOWN, "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "ellie"})))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(ME))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"username": "ellie"}))
                .append_header(AVAILABLE, "5"),
        )
        .with_priority(5)
        .mount(&server)
        .await;
    server
}

/// A capability reader for `server` whose eager warm-up fetch has finished.
async fn warm_cap_client(server: &MockServer) -> Arc<CapClient> {
    let base = Url::parse(&server.uri()).unwrap();
    let caps = Client::from_http(base, reqwest::Client::new()).unwrap().cap_client();
    // `get_server` waits for the warm-up fetch.
    caps.get_server::<CapabilitiesCap>().await.unwrap();
    caps
}

fn negotiating_client(server: &MockServer, caps: Arc<CapClient>, mode: CapMismatch) -> Client {
    let base = Url::parse(&server.uri()).unwrap();
    Client::from_http(base, reqwest::Client::new()).unwrap().with_capabilities(caps, mode)
}

async fn caps_hits(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == CAPABILITIES)
        .count()
}

/// Wait (bounded) for the background refresh to cache `token`, failing with what it saw.
async fn await_token(caps: &CapClient, token: &str) {
    for _ in 0..200 {
        if caps.known_token().as_deref() == Some(token) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the refresh never cached {token:?}; the cache holds {:?}", caps.known_token());
}

#[rstest]
#[tokio::test]
async fn concurrent_burst_refreshes_capabilities_once(#[future] negotiating_server: MockServer) {
    let server = negotiating_server.await;
    let caps = warm_cap_client(&server).await;
    let client = negotiating_client(&server, Arc::clone(&caps), CapMismatch::Continue);

    let mut handles = Vec::new();
    for _ in 0..20 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client.get_me().map_api_error().await.map(|me| me.status())
        }));
    }
    for handle in handles {
        assert_eq!(handle.await.unwrap().unwrap(), StatusCode::OK);
    }

    await_token(&caps, "5").await;
    assert_eq!(caps_hits(&server).await, 2, "a burst must coalesce into one refresh");
}
