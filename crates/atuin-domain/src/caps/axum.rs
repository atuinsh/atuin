//! Axum integration for capability negotiation: the capabilities endpoint, the negotiation
//! middleware, and a [`Router`] extension that installs it. Gated behind the `axum` feature.
//!
//! The handlers take the [`CapServer`] as axum state, so they are independent of any database or
//! app state. In a router the endpoint gets it via `with_state` and the middleware via
//! [`CapabilitiesRouterExt::negotiate_capabilities`].

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::http::{AVAILABLE_HEADER, ENFORCE_HEADER, KNOWN_HEADER};
use super::{CapServer, Negotiation};

/// `GET /api/v0/capabilities` -- serve the pre-serialized capability document.
#[allow(clippy::unused_async, reason = "must be async to implement axum::handler::Handler")]
pub async fn get(State(caps): State<Arc<CapServer>>) -> Response {
    ([(CONTENT_TYPE, HeaderValue::from_static("application/json"))], caps.body().to_owned())
        .into_response()
}

/// Capability-negotiation middleware for the sync API.
///
/// On a stale `X-Atuin-Capabilities-Known` token, the response depends on whether the request also
/// carries `X-Atuin-Capabilities-Enforce`:
/// - present -> reject with `412` + `X-Atuin-Capabilities-Available`;
/// - absent -> serve the request anyway, attaching `X-Atuin-Capabilities-Available` so the client
///   can refresh.
///
/// Absent or matching tokens pass straight through, so pre-capabilities clients are never affected.
/// A non-UTF-8 known header is treated as absent.
async fn check_token(State(caps): State<Arc<CapServer>>, request: Request, next: Next) -> Response {
    let enforce = request.headers().contains_key(ENFORCE_HEADER);
    let negotiation = {
        let known = request.headers().get(KNOWN_HEADER).and_then(|value| value.to_str().ok());
        caps.negotiate(known)
    };

    // The token is precomputed ASCII hex, so `from_str` never fails; guard rather than panic.
    let available = HeaderValue::from_str(caps.token()).ok();

    match negotiation {
        Negotiation::Current => next.run(request).await,
        Negotiation::Stale => {
            let mut response = if enforce {
                (StatusCode::PRECONDITION_FAILED, "capabilities out of date").into_response()
            } else {
                // The client did not ask us to enforce, so serve the request and tell it our token.
                next.run(request).await
            };
            if let Some(value) = available {
                response.headers_mut().insert(HeaderName::from_static(AVAILABLE_HEADER), value);
            }
            response
        }
    }
}

/// Install capability negotiation onto an axum [`Router`].
pub trait CapabilitiesRouterExt {
    /// Layer the capability-negotiation middleware onto this router, rejecting requests whose
    /// capability token is stale.
    #[must_use]
    fn negotiate_capabilities(self, caps: Arc<CapServer>) -> Self;
}

impl<S> CapabilitiesRouterExt for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn negotiate_capabilities(self, caps: Arc<CapServer>) -> Self {
        self.layer(axum::middleware::from_fn_with_state(caps, check_token))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, Request, StatusCode};
    use axum::routing::get as axum_get;
    use rstest::{fixture, rstest};
    use tower::ServiceExt;

    use super::{CapabilitiesRouterExt, get};
    use crate::caps::CapServer;
    use crate::caps::http::{AVAILABLE_HEADER, ENFORCE_HEADER, KNOWN_HEADER}; // oneshot

    /// An empty capability set -- advertises nothing, but still issues a stable token.
    #[fixture]
    fn caps() -> Arc<CapServer> {
        Arc::new(CapServer::new())
    }

    #[rstest]
    #[tokio::test]
    async fn endpoint_serves_the_document(caps: Arc<CapServer>) {
        let app: Router =
            Router::new().route("/api/v0/capabilities", axum_get(get)).with_state(caps.clone());

        let resp = app
            .oneshot(Request::builder().uri("/api/v0/capabilities").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/json");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), caps.body().as_bytes());
    }

    fn negotiating_app(caps: Arc<CapServer>) -> Router {
        Router::new().route("/probe", axum_get(|| async { "ok" })).negotiate_capabilities(caps)
    }

    #[rstest]
    #[tokio::test]
    async fn absent_known_header_passes(caps: Arc<CapServer>) {
        let resp = negotiating_app(caps)
            .oneshot(Request::builder().uri("/probe").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[rstest]
    #[tokio::test]
    async fn matching_token_passes(caps: Arc<CapServer>) {
        let resp = negotiating_app(caps.clone())
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(KNOWN_HEADER, caps.token())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[rstest]
    #[tokio::test]
    async fn non_utf8_known_header_is_treated_as_absent_and_passes(caps: Arc<CapServer>) {
        // `to_str().ok()` turns invalid UTF-8 into `None`, i.e. "no token known" -- not a 412,
        // and not a panic.
        let value = HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap();
        let resp = negotiating_app(caps)
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(KNOWN_HEADER, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[rstest]
    #[tokio::test]
    async fn stale_token_with_enforce_rejects_with_available_header(caps: Arc<CapServer>) {
        let resp = negotiating_app(caps.clone())
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(KNOWN_HEADER, "deadbeefdeadbeef")
                    .header(ENFORCE_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(resp.headers().get(AVAILABLE_HEADER).unwrap().to_str().unwrap(), caps.token());
    }

    #[rstest]
    #[tokio::test]
    async fn stale_token_without_enforce_proceeds_with_available_header(caps: Arc<CapServer>) {
        let resp = negotiating_app(caps.clone())
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(KNOWN_HEADER, "deadbeefdeadbeef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // No enforce header -> the request is served despite the stale token...
        assert_eq!(resp.status(), StatusCode::OK);
        // ...but the server still advertises its token so the client can refresh.
        assert_eq!(resp.headers().get(AVAILABLE_HEADER).unwrap().to_str().unwrap(), caps.token());
    }
}
