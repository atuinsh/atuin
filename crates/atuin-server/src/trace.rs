use std::net::SocketAddr;

use axum::extract::{ConnectInfo, MatchedPath, Request};
use tracing::{Span, field};

/// Build the root tracing span for an incoming HTTP request.
///
/// The span records the request method, matched route, and connecting client
/// IP, so that every event emitted while the request is handled inherits that
/// context. Health checks are recorded at `DEBUG` to keep the logs readable
/// under load-balancer polling; every other route is recorded at `INFO`.
pub fn make_request_span(request: &Request) -> Span {
    let method = request.method();

    // Prefer the matched route template (eg `/user/{username}`) over the raw
    // URI so the field stays low-cardinality.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| request.uri().path(), MatchedPath::as_str);

    let client_ip =
        request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(addr)| addr.ip());

    let span = if route.ends_with("/healthz") {
        tracing::debug_span!(
            "http.request",
            http.method = %method,
            http.route = route,
            client.ip = field::Empty,
        )
    } else {
        tracing::info_span!(
            "http.request",
            http.method = %method,
            http.route = route,
            client.ip = field::Empty,
        )
    };

    if let Some(ip) = client_ip {
        span.record("client.ip", field::display(ip));
    }

    span
}
