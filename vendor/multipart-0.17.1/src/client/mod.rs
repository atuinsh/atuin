// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! The client-side abstraction for multipart requests. Enabled with the `client` feature.
//!
//! Use this when sending POST requests with files to a server.
use mime::Mime;

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::io::prelude::*;

use std::path::Path;

#[cfg(feature = "hyper")]
pub mod hyper;

pub mod lazy;

mod sized;

pub use self::sized::SizedRequest;

const BOUNDARY_LEN: usize = 16;

macro_rules! map_self {
    ($selff:expr, $try:expr) => {
        match $try {
            Ok(_) => Ok($selff),
            Err(err) => Err(err.into()),
        }
    };
}

/// The entry point of the client-side multipart API.
///
/// Though they perform I/O, the `.write_*()` methods do not return `io::Result<_>` in order to
/// facilitate method chaining. Upon the first error, all subsequent API calls will be no-ops until
/// `.send()` is called, at which point the error will be reported.
pub struct Multipart<S> {
    writer: MultipartWriter<'static, S>,
}

impl Multipart<()> {
    /// Create a new `Multipart` to wrap a request.
    ///
    /// ## Returns Error
    /// If `req.open_stream()` returns an error.
    pub fn from_request<R: HttpRequest>(req: R) -> Result<Multipart<R::Stream>, R::Error> {
        let (boundary, stream) = open_stream(req, None)?;

        Ok(Multipart {
            writer: MultipartWriter::new(stream, boundary),
        })
    }
}

impl<S: HttpStream> Multipart<S> {
    /// Write a text field to this multipart request.
    /// `name` and `val` can be either owned `String` or `&str`.
    ///
    /// ## Errors
    /// If something went wrong with the HTTP stream.
    pub fn write_text<N: AsRef<str>, V: AsRef<str>>(
        &mut self,
        name: N,
        val: V,
    ) -> Result<&mut Self, S::Error> {
        map_self!(self, self.writer.write_text(name.as_ref(), val.as_ref()))
    }

    /// Open a file pointed to by `path` and write its contents to the multipart request,
    /// supplying its filename and guessing its `Content-Type` from its extension.
    ///
    /// If you want to set these values manually, or use another type that implements `Read`,
    /// use `.write_stream()`.
    ///
    /// `name` can be either `String` or `&str`, and `path` can be `PathBuf` or `&Path`.
    ///
    /// ## Errors
    /// If there was a problem opening the file (was a directory or didn't exist),
    /// or if something went wrong with the HTTP stream.
    pub fn write_file<N: AsRef<str>, P: AsRef<Path>>(
        &mut self,
        name: N,
        path: P,
    ) -> Result<&mut Self, S::Error> {
        let name = name.as_ref();
        let path = path.as_ref();

        map_self!(self, self.writer.write_file(name, path))
    }

    /// Write a byte stream to the multipart request as a file field, supplying `filename` if given,
    /// and `content_type` if given or `"application/octet-stream"` if not.
    ///
    /// `name` can be either `String` or `&str`, and `read` can take the `Read` by-value or
    /// with an `&mut` borrow.
    ///
    /// ## Warning
    /// The given `Read` **must** be able to read to EOF (end of file/no more data), meaning
    /// `Read::read()` returns `Ok(0)`. If it never returns EOF it will be read to infinity
    /// and the request will never be completed.
    ///
    /// When using `SizedRequest` this also can cause out-of-control memory usage as the
    /// multipart data has to be written to an in-memory buffer so its size can be calculated.
    ///
    /// Use `Read::take()` if you wish to send data from a `Read`
    /// that will never return EOF otherwise.
    ///
    /// ## Errors
    /// If the reader returned an error, or if something went wrong with the HTTP stream.
    // RFC: How to format this declaration?
    pub fn write_stream<N: AsRef<str>, St: Read>(
        &mut self,
        name: N,
        stream: &mut St,
        filename: Option<&str>,
        content_type: Option<Mime>,
    ) -> Result<&mut Self, S::Error> {
        let name = name.as_ref();

        map_self!(
            self,
            self.writer
                .write_stream(stream, name, filename, content_type)
        )
    }

    /// Finalize the request and return the response from the server, or the last error if set.
    pub fn send(self) -> Result<S::Response, S::Error> {
        self.writer
            .finish()
            .map_err(io::Error::into)
            .and_then(|body| body.finish())
    }
}

impl<R: HttpRequest> Multipart<SizedRequest<R>>
where
    <R::Stream as HttpStream>::Error: From<R::Error>,
{
    /// Create a new `Multipart` using the `SizedRequest` wrapper around `req`.
    pub fn from_request_sized(req: R) -> Result<Self, R::Error> {
        Multipart::from_request(SizedRequest::from_request(req))
    }
}

/// A trait describing an HTTP request that can be used to send multipart data.
pub trait HttpRequest {
    /// The HTTP stream type that can be opend by this request, to which the multipart data will be
    /// written.
    type Stream: HttpStream;
    /// The error type for this request.
    /// Must be compatible with `io::Error` as well as `Self::HttpStream::Error`
    type Error: From<io::Error> + Into<<Self::Stream as HttpStream>::Error>;

    /// Set the `Content-Type` header to `multipart/form-data` and supply the `boundary` value.
    /// If `content_len` is given, set the `Content-Length` header to its value.
    ///
    /// Return `true` if any and all sanity checks passed and the stream is ready to be opened,
    /// or `false` otherwise.
    fn apply_headers(&mut self, boundary: &str, content_len: Option<u64>) -> bool;

    /// Open the request stream and return it or any error otherwise.
    fn open_stream(self) -> Result<Self::Stream, Self::Error>;
}

/// A trait describing an open HTTP stream that can be written to.
pub trait HttpStream: Write {
    /// The request type that opened this stream.
    type Request: HttpRequest;
    /// The response type that will be returned after the request is completed.
    type Response;
    /// The error type for this stream.
    /// Must be compatible with `io::Error` as well as `Self::Request::Error`.
    type Error: From<io::Error> + From<<Self::Request as HttpRequest>::Error>;

    /// Finalize and close the stream and return the response object, or any error otherwise.
    fn finish(self) -> Result<Self::Response, Self::Error>;
}

impl HttpRequest for () {
    type Stream = io::Sink;
    type Error = io::Error;

    fn apply_headers(&mut self, _: &str, _: Option<u64>) -> bool {
        true
    }
    fn open_stream(self) -> Result<Self::Stream, Self::Error> {
        Ok(io::sink())
    }
}

impl HttpStream for io::Sink {
    type Request = ();
    type Response = ();
    type Error = io::Error;

    fn finish(self) -> Result<Self::Response, Self::Error> {
        Ok(())
    }
}

fn gen_boundary() -> String {
    ::random_alphanumeric(BOUNDARY_LEN)
}

fn open_stream<R: HttpRequest>(
    mut req: R,
    content_len: Option<u64>,
) -> Result<(String, R::Stream), R::Error> {
    let boundary = gen_boundary();
    req.apply_headers(&boundary, content_len);
    req.open_stream().map(|stream| (boundary, stream))
}

struct MultipartWriter<'a, W> {
    inner: W,
    boundary: Cow<'a, str>,
    data_written: bool,
}

impl<'a, W: Write> MultipartWriter<'a, W> {
    fn new<B: Into<Cow<'a, str>>>(inner: W, boundary: B) -> Self {
        MultipartWriter {
            inner,
            boundary: boundary.into(),
            data_written: false,
        }
    }

    fn write_boundary(&mut self) -> io::Result<()> {
        if self.data_written {
            self.inner.write_all(b"\r\n")?;
        }

        write!(self.inner, "--{}\r\n", self.boundary)
    }

    fn write_text(&mut self, name: &str, text: &str) -> io::Result<()> {
        chain_result! {
            self.write_field_headers(name, None, None),
            self.inner.write_all(text.as_bytes())
        }
    }

    fn write_file(&mut self, name: &str, path: &Path) -> io::Result<()> {
        let (content_type, filename) = mime_filename(path);
        let mut file = File::open(path)?;
        self.write_stream(&mut file, name, filename, Some(content_type))
    }

    fn write_stream<S: Read>(
        &mut self,
        stream: &mut S,
        name: &str,
        filename: Option<&str>,
        content_type: Option<Mime>,
    ) -> io::Result<()> {
        // This is necessary to make sure it is interpreted as a file on the server end.
        let content_type = Some(content_type.unwrap_or(mime::APPLICATION_OCTET_STREAM));

        chain_result! {
            self.write_field_headers(name, filename, content_type),
            io::copy(stream, &mut self.inner),
            Ok(())
        }
    }

    fn write_field_headers(
        &mut self,
        name: &str,
        filename: Option<&str>,
        content_type: Option<Mime>,
    ) -> io::Result<()> {
        chain_result! {
            // Write the first boundary, or the boundary for the previous field.
            self.write_boundary(),
            { self.data_written = true; Ok(()) },
            write!(self.inner, "Content-Disposition: form-data; name=\"{}\"", name),
            filename.map(|filename| write!(self.inner, "; filename=\"{}\"", filename))
                .unwrap_or(Ok(())),
            content_type.map(|content_type| write!(self.inner, "\r\nContent-Type: {}", content_type))
                .unwrap_or(Ok(())),
            self.inner.write_all(b"\r\n\r\n")
        }
    }

    fn finish(mut self) -> io::Result<W> {
        if self.data_written {
            self.inner.write_all(b"\r\n")?;
        }

        // always write the closing boundary, even for empty bodies
        // trailing CRLF is optional but Actix requires it due to a naive implementation:
        // https://github.com/actix/actix-web/issues/598
        write!(self.inner, "--{}--\r\n", self.boundary)?;
        Ok(self.inner)
    }
}

fn mime_filename(path: &Path) -> (Mime, Option<&str>) {
    let content_type = ::mime_guess::from_path(path);
    let filename = opt_filename(path);
    (content_type.first_or_octet_stream(), filename)
}

fn opt_filename(path: &Path) -> Option<&str> {
    path.file_name().and_then(|filename| filename.to_str())
}
