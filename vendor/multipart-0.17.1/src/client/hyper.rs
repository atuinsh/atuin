// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Client-side integration with [Hyper](https://github.com/hyperium/hyper). 
//! Enabled with the `hyper` feature (on by default).
//!
//! Contains `impl HttpRequest for Request<Fresh>` and `impl HttpStream for Request<Streaming>`.
//!
//! Also see: [`lazy::Multipart::client_request()`](../lazy/struct.Multipart.html#method.client_request)
//! and [`lazy::Multipart::client_request_mut()`](../lazy/struct.Multipart.html#method.client_request_mut)
//! (adaptors for `hyper::client::RequestBuilder`).
use hyper::client::request::Request;
use hyper::client::response::Response;
use hyper::header::{ContentType, ContentLength};
use hyper::method::Method;
use hyper::net::{Fresh, Streaming};

use hyper::Error as HyperError;

use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};

use super::{HttpRequest, HttpStream};

/// #### Feature: `hyper`
impl HttpRequest for Request<Fresh> {
    type Stream = Request<Streaming>;
    type Error = HyperError;

    /// # Panics
    /// If `self.method() != Method::Post`.
    fn apply_headers(&mut self, boundary: &str, content_len: Option<u64>) -> bool {
        if self.method() != Method::Post {
            error!(
                "Expected Hyper request method to be `Post`, was actually `{:?}`",
                self.method()
            );

            return false;
        }

        let headers = self.headers_mut();

        headers.set(ContentType(multipart_mime(boundary)));

        if let Some(size) = content_len {
            headers.set(ContentLength(size));   
        }

        debug!("Hyper headers: {}", headers); 

        true
    }

    fn open_stream(self) -> Result<Self::Stream, Self::Error> {
        self.start()
    }
} 

/// #### Feature: `hyper`
impl HttpStream for Request<Streaming> {
    type Request = Request<Fresh>;
    type Response = Response;
    type Error = HyperError;

    fn finish(self) -> Result<Self::Response, Self::Error> {
        self.send()
    }
}

/// Create a `Content-Type: multipart/form-data;boundary={bound}`
pub fn content_type(bound: &str) -> ContentType {
    ContentType(multipart_mime(bound))
}

fn multipart_mime(bound: &str) -> Mime {
    Mime(
        TopLevel::Multipart, SubLevel::Ext("form-data".into()),
        vec![(Attr::Ext("boundary".into()), Value::Ext(bound.into()))]
    )         
}
