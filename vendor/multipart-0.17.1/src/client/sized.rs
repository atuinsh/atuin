// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Sized/buffered wrapper around `HttpRequest`.

use client::{HttpRequest, HttpStream};

use std::io;
use std::io::prelude::*;

/// A wrapper around a request object that measures the request body and sets the `Content-Length`
/// header to its size in bytes.
///
/// Sized requests are more human-readable and use less bandwidth 
/// (as chunking adds [visual noise and overhead][chunked-example]),
/// but they must be able to load their entirety, including the contents of all files
/// and streams, into memory so the request body can be measured.
///
/// You should really only use sized requests if you intend to inspect the data manually on the
/// server side, as it will produce a more human-readable request body. Also, of course, if the
/// server doesn't support chunked requests or otherwise rejects them. 
///
/// [chunked-example]: http://en.wikipedia.org/wiki/Chunked_transfer_encoding#Example 
pub struct SizedRequest<R> {
    inner: R,
    buffer: Vec<u8>,
    boundary: String,
}

impl<R: HttpRequest> SizedRequest<R> {
    #[doc(hidden)]
    pub fn from_request(req: R) -> SizedRequest<R> {
        SizedRequest {
            inner: req,
            buffer: Vec::new(),
            boundary: String::new(),
        }
    }
}

impl<R> Write for SizedRequest<R> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buffer.write(data)
    }

    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl<R: HttpRequest> HttpRequest for SizedRequest<R> 
where <R::Stream as HttpStream>::Error: From<R::Error> {
    type Stream = Self;
    type Error = R::Error;

    /// `SizedRequest` ignores `_content_len` because it sets its own later.
    fn apply_headers(&mut self, boundary: &str, _content_len: Option<u64>) -> bool {
        self.boundary.clear();
        self.boundary.push_str(boundary);
        true
    }

    fn open_stream(mut self) -> Result<Self, Self::Error> {
        self.buffer.clear();
        Ok(self)
    }
}

impl<R: HttpRequest> HttpStream for SizedRequest<R> 
where <R::Stream as HttpStream>::Error: From<R::Error> { 
    type Request = Self;
    type Response = <<R as HttpRequest>::Stream as HttpStream>::Response;
    type Error = <<R as HttpRequest>::Stream as HttpStream>::Error;

    fn finish(mut self) -> Result<Self::Response, Self::Error> {
        let content_len = self.buffer.len() as u64;
        
        if !self.inner.apply_headers(&self.boundary, Some(content_len)) {
            return Err(io::Error::new(
                io::ErrorKind::Other, 
                "SizedRequest failed to apply headers to wrapped request."
            ).into());
        }

        let mut req = self.inner.open_stream()?;
        io::copy(&mut &self.buffer[..], &mut req)?;
        req.finish()
    }
}
