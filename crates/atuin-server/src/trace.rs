use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, MatchedPath, Request};
use axum::response::Response;
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
        // recorded by `on_response` once the response is ready
        http.status_code = field::Empty,
        client.ip = field::Empty,
    );

    if let Some(ip) = client_ip {
        span.record("client.ip", field::display(ip));
    }

    span
}

/// Record the response status on the request span and emit the access-log line,
/// under the `atuin_server` target so it shows with the default
/// `atuin_server=info` filter.
pub fn on_response(response: &Response, latency: Duration, span: &Span) {
    span.record("http.status_code", response.status().as_u16());
    tracing::info!(latency_ms = latency.as_millis(), "request completed");
}
