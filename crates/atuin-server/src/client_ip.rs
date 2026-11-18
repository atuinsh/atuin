//! Resolution of the real client address for requests that arrive through a
//! reverse proxy.
//!
//! When Atuin runs behind a load balancer the TCP peer is the proxy, not the
//! user. Proxies pass the original address along in a header such as
//! `X-Forwarded-For` or `X-Real-IP`, but anyone can send that header, so it is
//! only honoured when the peer itself is listed in `trusted_proxies`. The
//! header is walked from right to left, skipping trusted hops, and the first
//! untrusted address is the client.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use atuin_domain::api::ErrorResponse;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use ipnet::IpNet;

use crate::handlers::{ErrorResponseStatus, RespExt};

/// The proxy networks whose client address header is honoured, and the name of that header.
#[derive(Clone, Debug)]
pub struct TrustedProxies {
    nets: Arc<[IpNet]>,
    header: HeaderName,
}

impl TrustedProxies {
    pub fn new(nets: impl Into<Arc<[IpNet]>>, header: HeaderName) -> Self {
        Self {
            nets: nets.into(),
            header,
        }
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(&ip))
    }
}

/// The address a request originated from, after trusted proxies are peeled off.
///
/// Inserted into request extensions by [`resolve`], and available to handlers as an extractor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, derive_more::Display)]
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = ErrorResponseStatus<'static>;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().copied().ok_or_else(|| {
            tracing::error!("client address is missing from the request");
            ErrorResponse::reply("could not determine client address")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)
        })
    }
}

/// Middleware that records the resolved [`ClientIp`] on the request.
pub async fn resolve(
    State(trusted): State<TrustedProxies>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());

    if let Some(peer) = peer {
        let ip = resolve_client_ip(peer, request.headers(), &trusted);
        request.extensions_mut().insert(ClientIp(ip));
    }

    next.run(request).await
}

fn resolve_client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> IpAddr {
    // A listener bound to `::` reports IPv4 peers as IPv4-mapped IPv6 addresses.
    let peer = peer.to_canonical();

    if !trusted.contains(peer) {
        return peer;
    }

    let forwarded = headers
        .get_all(&trusted.header)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();

    // The rightmost entry was added by the proxy we trust, the one before it by
    // whoever talked to that proxy, and so on. The first hop that is not one of
    // our proxies is the client; anything further left is unverifiable.
    for entry in forwarded.iter().rev() {
        let Ok(ip) = entry.parse::<IpAddr>() else {
            return peer;
        };
        let ip = ip.to_canonical();
        if !trusted.contains(ip) {
            return ip;
        }
    }

    // Every hop was a trusted proxy, so the leftmost one is as close to the
    // client as we can get.
    forwarded.first().and_then(|entry| entry.parse().ok()).unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
    use axum::routing::get;
    use ipnet::IpNet;
    use rstest::{fixture, rstest};
    use tower::ServiceExt;

    use super::{ClientIp, TrustedProxies, resolve, resolve_client_ip};

    const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
    const X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");

    fn trusted_with_header(header: HeaderName) -> TrustedProxies {
        TrustedProxies::new(
            vec![
                "172.18.0.0/16".parse::<IpNet>().unwrap(),
                "10.0.0.1/32".parse::<IpNet>().unwrap(),
                "fd00::/8".parse::<IpNet>().unwrap(),
            ],
            header,
        )
    }

    #[fixture]
    fn trusted() -> TrustedProxies {
        trusted_with_header(X_FORWARDED_FOR)
    }

    fn headers(forwarded_for: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in forwarded_for {
            headers.append(X_FORWARDED_FOR, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[rstest]
    #[case::no_header("172.18.0.1", &[], "172.18.0.1")]
    #[case::single_hop("172.18.0.1", &["203.0.113.7"], "203.0.113.7")]
    #[case::chain_of_trusted_hops("172.18.0.1", &["203.0.113.7, 10.0.0.1, 172.18.0.2"], "203.0.113.7")]
    #[case::stops_at_first_untrusted("172.18.0.1", &["198.51.100.9, 203.0.113.7, 10.0.0.1"], "203.0.113.7")]
    #[case::multiple_headers("172.18.0.1", &["203.0.113.7", "10.0.0.1"], "203.0.113.7")]
    #[case::whitespace_and_empty_entries("172.18.0.1", &[" 203.0.113.7 , , 10.0.0.1 "], "203.0.113.7")]
    #[case::all_hops_trusted("172.18.0.1", &["10.0.0.1, 172.18.0.2"], "10.0.0.1")]
    #[case::garbage_entry("172.18.0.1", &["203.0.113.7, not-an-ip"], "172.18.0.1")]
    #[case::empty_header("172.18.0.1", &[""], "172.18.0.1")]
    #[case::untrusted_peer_ignores_header("203.0.113.7", &["198.51.100.9"], "203.0.113.7")]
    #[case::ipv4_mapped_peer("::ffff:172.18.0.1", &["203.0.113.7"], "203.0.113.7")]
    #[case::ipv4_mapped_hop("172.18.0.1", &["::ffff:203.0.113.7"], "203.0.113.7")]
    #[case::ipv6_peer("fd00::1", &["2001:db8::9"], "2001:db8::9")]
    fn resolves_client_ip(
        trusted: TrustedProxies,
        #[case] peer: IpAddr,
        #[case] forwarded_for: &[&str],
        #[case] expected: IpAddr,
    ) {
        let resolved = resolve_client_ip(peer, &headers(forwarded_for), &trusted);
        assert_eq!(resolved, expected);
    }

    #[rstest]
    #[case::reads_configured_header(X_REAL_IP, "203.0.113.7", "203.0.113.7")]
    #[case::ignores_other_headers(X_FORWARDED_FOR, "203.0.113.7", "172.18.0.1")]
    fn only_the_configured_header_is_read(
        #[case] configured: HeaderName,
        #[case] real_ip: &str,
        #[case] expected: IpAddr,
    ) {
        let trusted = trusted_with_header(configured);
        let peer: IpAddr = "172.18.0.1".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(X_REAL_IP, HeaderValue::from_str(real_ip).unwrap());

        assert_eq!(resolve_client_ip(peer, &headers, &trusted), expected);
    }

    #[rstest]
    fn no_trusted_proxies_always_uses_peer() {
        let trusted = TrustedProxies::new(Vec::new(), X_FORWARDED_FOR);
        let peer: IpAddr = "172.18.0.1".parse().unwrap();
        let resolved = resolve_client_ip(peer, &headers(&["203.0.113.7"]), &trusted);
        assert_eq!(resolved, peer);
    }

    async fn echo(ClientIp(ip): ClientIp) -> String {
        ip.to_string()
    }

    fn app(trusted: TrustedProxies) -> Router {
        Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn_with_state(trusted, resolve))
    }

    #[rstest]
    #[tokio::test]
    async fn middleware_exposes_resolved_ip_to_handlers(trusted: TrustedProxies) {
        let peer: SocketAddr = "172.18.0.1:4242".parse().unwrap();
        let request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "203.0.113.7")
            .extension(ConnectInfo(peer))
            .body(Body::empty())
            .unwrap();

        let response = app(trusted).oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(body, "203.0.113.7");
    }

    #[rstest]
    #[tokio::test]
    async fn missing_connect_info_is_a_server_error(trusted: TrustedProxies) {
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();

        let response = app(trusted).oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
