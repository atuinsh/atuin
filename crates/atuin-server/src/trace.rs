use std::net::SocketAddr;

use axum::extract::{ConnectInfo, MatchedPath, Request};
use tracing::{Span, field};

/// Build the root tracing span for an incoming HTTP request.
///
/// The span records the request method, matched route, and connecting client
/// IP at `INFO`, so that every event emitted while the request is handled
/// inherits that context and the request is visible under the default
/// `atuin_server=info` filter. `/healthz` is kept out of the logs by omitting
/// it from the trace layer entirely (see [`crate::router`]), not by lowering
/// its span level here.
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

#[cfg(test)]
mod tests {
    use atuin_common::test_utils::capture_logs;
    use axum::http::Request;
    use tracing::Level;

    use super::make_request_span;

    /// Every route, including `/healthz`, is recorded at INFO. Healthz is kept
    /// out of the access log by being omitted from the trace layer entirely (see
    /// `router`), not by downgrading its span level.
    #[test]
    fn request_spans_are_info_level() {
        let _logs = capture_logs();

        for uri in ["/api/v0/record", "/healthz"] {
            let request = Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap();
            let span = make_request_span(&request);
            let metadata = span.metadata().expect("span should be enabled under a subscriber");

            assert_eq!(*metadata.level(), Level::INFO, "{uri} should be an INFO span");
            assert_eq!(metadata.name(), "http.request");
        }
    }
}
