//! Support for `multipart/form-data` bodies in [Nickel](https://nickel.rs).
pub extern crate nickel;

use self::nickel::hyper;
use self::hyper::header::ContentType;

pub use self::nickel::Request as NickelRequest;
pub use self::nickel::hyper::server::Request as HyperRequest;

use server::{HttpRequest, Multipart};

/// A wrapper for `&mut nickel::Request` which implements `multipart::server::HttpRequest`.
///
/// Necessary because this crate cannot directly provide an impl of `HttpRequest` for
/// `&mut NickelRequest`.
pub struct Maybe<'r, 'mw: 'r, 'server: 'mw, D: 'mw>(pub &'r mut NickelRequest<'mw, 'server, D>);

impl<'r, 'mw: 'r, 'server: 'mw, D: 'mw> HttpRequest for Maybe<'r, 'mw, 'server, D> {
    type Body = &'r mut HyperRequest<'mw, 'server>;

    fn multipart_boundary(&self) -> Option<&str> {
        // we can't use the impl from the `hyper` module because it might be the wrong version
        let cont_type = try_opt!(self.0.origin.headers.get::<ContentType>());
        cont_type.get_param("boundary").map(|v| v.as_str())
    }

    fn body(self) -> Self::Body {
        &mut self.0.origin
    }
}

/// Extension trait for getting the `multipart/form-data` body from `nickel::Request`.
///
/// Implemented for `nickel::Request`.
pub trait MultipartBody<'mw, 'server> {
    /// Get a multipart reader for the request body, if the request is of the right type.
    fn multipart_body(&mut self) -> Option<Multipart<&mut HyperRequest<'mw, 'server>>>;
}

impl<'mw, 'server, D: 'mw> MultipartBody<'mw, 'server> for NickelRequest<'mw, 'server, D> {
    fn multipart_body(&mut self) -> Option<Multipart<&mut HyperRequest<'mw, 'server>>> {
        Multipart::from_request(Maybe(self)).ok()
    }
}

impl<'r, 'mw: 'r, 'server: 'mw, D: 'mw> AsRef<&'r mut NickelRequest<'mw, 'server, D>> for Maybe<'r, 'mw, 'server, D> {
    fn as_ref(&self) -> &&'r mut NickelRequest<'mw, 'server, D> {
        &self.0
    }
}

impl<'r, 'mw: 'r, 'server: 'mw, D: 'mw> AsMut<&'r mut NickelRequest<'mw, 'server, D>> for Maybe<'r, 'mw, 'server, D> {
    fn as_mut(&mut self) -> &mut &'r mut NickelRequest<'mw, 'server, D> {
        &mut self.0
    }
}

impl<'r, 'mw: 'r, 'server: 'mw, D: 'mw> Into<&'r mut NickelRequest<'mw, 'server, D>> for Maybe<'r, 'mw, 'server, D> {
    fn into(self) -> &'r mut NickelRequest<'mw, 'server, D> {
        self.0
    }
}

impl<'r, 'mw: 'r, 'server: 'mw, D: 'mw> From<&'r mut NickelRequest<'mw, 'server, D>> for Maybe<'r, 'mw, 'server, D> {
    fn from(req: &'r mut NickelRequest<'mw, 'server, D>) -> Self {
        Maybe(req)
    }
}

