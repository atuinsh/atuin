use std::net::SocketAddr;

use axum::extract::{ConnectInfo, MatchedPath, Request};
use tracing::{Span, field};

/// Build the root tracing span for an incoming HTTP request.
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

    let span = tracing::info_span!(
        "http.request",
        http.method = %method,
        http.route = route,
        client.ip = field::Empty,
    );

    if let Some(ip) = client_ip {
        span.record("client.ip", field::display(ip));
    }

    span
}
