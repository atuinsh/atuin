// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Server-side integration with [Hyper](https://github.com/hyperium/hyper).
//! Enabled with the `hyper` feature (on by default).
//!
//! Also contains an implementation of [`HttpRequest`](../trait.HttpRequest.html)
//! for `hyper::server::Request` and `&mut hyper::server::Request`.
use hyper::net::Fresh;
use hyper::header::ContentType;
use hyper::method::Method;
use hyper::server::{Handler, Request, Response};

pub use hyper::server::Request as HyperRequest;

use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};

use super::{Multipart, HttpRequest};

/// A container that implements `hyper::server::Handler` which will switch
/// the handler implementation depending on if the incoming request is multipart or not.
///
/// Create an instance with `new()` and pass it to `hyper::server::Server::listen()` where
/// you would normally pass a `Handler` instance.
///
/// A convenient wrapper for `Multipart::from_request()`.
pub struct Switch<H, M> {
    normal: H,
    multipart: M,
}

impl<H, M> Switch<H, M> where H: Handler, M: MultipartHandler {
    /// Create a new `Switch` instance where
    /// `normal` handles normal Hyper requests and `multipart` handles Multipart requests
    pub fn new(normal: H, multipart: M) -> Switch<H, M> {
        Switch { normal, multipart }
    }
}

impl<H, M> Handler for Switch<H, M> where H: Handler, M: MultipartHandler {
    fn handle<'a, 'k>(&'a self, req: Request<'a, 'k>, res: Response<'a, Fresh>) {
        match Multipart::from_request(req) {
            Ok(multi) => self.multipart.handle_multipart(multi, res),
            Err(req) => self.normal.handle(req, res),
        }
    }
}

/// A trait defining a type that can handle an incoming multipart request.
///
/// Extends to closures of the type `Fn(Multipart<Request>, Response<Fresh>)`,
/// and subsequently static functions.
pub trait MultipartHandler: Send + Sync {
    /// Generate a response from this multipart request.
    fn handle_multipart<'a, 'k>(&self, 
                                multipart: Multipart<Request<'a, 'k>>, 
                                response: Response<'a, Fresh>);
}

impl<F> MultipartHandler for F 
where F: Fn(Multipart<Request>, Response<Fresh>), F: Send + Sync {
    fn handle_multipart<'a, 'k>(&self, 
                                multipart: Multipart<Request<'a, 'k>>, 
                                response: Response<'a, Fresh>) {
        (*self)(multipart, response);
    }
}

impl<'a, 'b> HttpRequest for HyperRequest<'a, 'b> {
    type Body = Self;

    fn multipart_boundary(&self) -> Option<&str> {
        if self.method != Method::Post {
            return None;
        }

        self.headers.get::<ContentType>().and_then(|ct| {
            let ContentType(ref mime) = *ct;
            let params = match *mime {
                Mime(TopLevel::Multipart, SubLevel::FormData, ref params) => params,
                _ => return None,
            };

            params.iter().find(|&&(ref name, _)|
                match *name {
                    Attr::Boundary => true,
                    _ => false,
                }
            ).and_then(|&(_, ref val)|
                match *val {
                    Value::Ext(ref val) => Some(&**val),
                    _ => None,
                }
            )
        })
    }

    fn body(self) -> Self {
        self
    }
}

impl<'r, 'a, 'b> HttpRequest for &'r mut HyperRequest<'a, 'b> {
    type Body = Self;

    fn multipart_boundary(&self) -> Option<&str> {
        if self.method != Method::Post {
            return None;
        }

        self.headers.get::<ContentType>().and_then(|ct| {
            let ContentType(ref mime) = *ct;
            let params = match *mime {
                Mime(TopLevel::Multipart, SubLevel::FormData, ref params) => params,
                _ => return None,
            };

            params.iter().find(|&&(ref name, _)|
                match *name {
                    Attr::Boundary => true,
                    _ => false,
                }
            ).and_then(|&(_, ref val)|
                match *val {
                    Value::Ext(ref val) => Some(&**val),
                    _ => None,
                }
            )
        })
    }

    fn body(self) -> Self::Body {
        self
    }
}

