//! Redirect requests to a new location.
//!
//! The types in this module are helpers that implement [`Reply`](Reply), and easy
//! to use in order to setup redirects.

use http::{header, StatusCode};

use self::sealed::AsLocation;
use crate::reply::{self, Reply};

/// A simple `301` permanent redirect to a different location.
///
/// # Example
///
/// ```
/// use warp::{http::Uri, Filter};
///
/// let route = warp::path("v1")
///     .map(|| {
///         warp::redirect(Uri::from_static("/v2"))
///     });
/// ```
pub fn redirect(uri: impl AsLocation) -> impl Reply {
    reply::with_header(
        StatusCode::MOVED_PERMANENTLY,
        header::LOCATION,
        uri.header_value(),
    )
}

/// A simple `303` redirect to a different location.
///
/// The HTTP method of the request to the new location will always be `GET`.
///
/// # Example
///
/// ```
/// use warp::{http::Uri, Filter};
///
/// let route = warp::path("v1")
///     .map(|| {
///         warp::redirect::see_other(Uri::from_static("/v2"))
///     });
/// ```
pub fn see_other(uri: impl AsLocation) -> impl Reply {
    reply::with_header(StatusCode::SEE_OTHER, header::LOCATION, uri.header_value())
}

/// A simple `307` temporary redirect to a different location.
///
/// This is similar to [`see_other`](fn@see_other) but the HTTP method and the body of the request
/// to the new location will be the same as the method and body of the current request.
///
/// # Example
///
/// ```
/// use warp::{http::Uri, Filter};
///
/// let route = warp::path("v1")
///     .map(|| {
///         warp::redirect::temporary(Uri::from_static("/v2"))
///     });
/// ```
pub fn temporary(uri: impl AsLocation) -> impl Reply {
    reply::with_header(
        StatusCode::TEMPORARY_REDIRECT,
        header::LOCATION,
        uri.header_value(),
    )
}

/// A simple `308` permanent redirect to a different location.
///
/// This is similar to [`redirect`](fn@redirect) but the HTTP method of the request to the new
/// location will be the same as the method of the current request.
///
/// # Example
///
/// ```
/// use warp::{http::Uri, Filter};
///
/// let route = warp::path("v1")
///     .map(|| {
///         warp::redirect::permanent(Uri::from_static("/v2"))
///     });
/// ```
pub fn permanent(uri: impl AsLocation) -> impl Reply {
    reply::with_header(
        StatusCode::PERMANENT_REDIRECT,
        header::LOCATION,
        uri.header_value(),
    )
}

mod sealed {
    use bytes::Bytes;
    use http::{header::HeaderValue, Uri};

    // These sealed traits are to allow adding possibly new impls so other
    // arguments could be accepted, like maybe just `warp::redirect("/v2")`.
    pub trait AsLocation: Sealed {}
    pub trait Sealed {
        fn header_value(self) -> HeaderValue;
    }

    impl AsLocation for Uri {}

    impl Sealed for Uri {
        fn header_value(self) -> HeaderValue {
            let bytes = Bytes::from(self.to_string());
            HeaderValue::from_maybe_shared(bytes).expect("Uri is a valid HeaderValue")
        }
    }
}
