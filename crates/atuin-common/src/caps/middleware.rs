//! Client-side reqwest middleware for HTTP capability negotiation.
//!
//! The middleware stamps the client's last-known capability token onto each request and, when the
//! server rejects with `412` plus a differing available token, refreshes capabilities (and, if so
//! configured, retries the request once).

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Request, Response, StatusCode};

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

#[cfg(test)]
mod tests {
    use super::*;

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
