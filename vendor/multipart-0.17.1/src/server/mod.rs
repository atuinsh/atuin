// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! The server-side abstraction for multipart requests. Enabled with the `server` feature.
//!
//! Use this when you are implementing an HTTP server and want to
//! to accept, parse, and serve HTTP `multipart/form-data` requests (file uploads).
//!
//! See the `Multipart` struct for more info.

pub extern crate buf_redux;
extern crate httparse;
extern crate twoway;

use std::borrow::Borrow;
use std::io::prelude::*;
use std::io;

use self::boundary::BoundaryReader;

use self::field::PrivReadEntry;

pub use self::field::{FieldHeaders, MultipartField, MultipartData, ReadEntry, ReadEntryResult};

use self::save::SaveBuilder;

pub use self::save::{Entries, SaveResult, SavedField};

macro_rules! try_opt (
    ($expr:expr) => (
        match $expr {
            Some(val) => val,
            None => return None,
        }
    );
    ($expr:expr, $before_ret:expr) => (
        match $expr {
            Some(val) => val,
            None => {
                $before_ret;
                return None;
            }
        }
    )
);

macro_rules! try_read_entry {
    ($self_:expr; $try:expr) => (
        match $try {
            Ok(res) => res,
            Err(err) => return ::server::ReadEntryResult::Error($self_, err),
        }
    )
}

mod boundary;
mod field;

#[cfg(feature = "hyper")]
pub mod hyper;

#[cfg(feature = "iron")]
pub mod iron;

#[cfg(feature = "tiny_http")]
pub mod tiny_http;

#[cfg(feature = "nickel")]
pub mod nickel;

pub mod save;

/// The server-side implementation of `multipart/form-data` requests.
///
/// Implements `Borrow<R>` to allow access to the request body, if desired.
pub struct Multipart<R> {
    reader: BoundaryReader<R>,
}

impl Multipart<()> {
    /// If the given `HttpRequest` is a multipart/form-data POST request,
    /// return the request body wrapped in the multipart reader. Otherwise,
    /// returns the original request.
    pub fn from_request<R: HttpRequest>(req: R) -> Result<Multipart<R::Body>, R> {
        //FIXME: move `map` expr to `Some` arm when nonlexical borrow scopes land.
        let boundary = match req.multipart_boundary().map(String::from) {
            Some(boundary) => boundary,
            None => return Err(req),
        };

        Ok(Multipart::with_body(req.body(), boundary))        
    }   
}

impl<R: Read> Multipart<R> {
    /// Construct a new `Multipart` with the given body reader and boundary.
    ///
    /// ## Note: `boundary`
    /// This will prepend the requisite `--` to the boundary string as documented in
    /// [IETF RFC 1341, Section 7.2.1: "Multipart: the common syntax"][rfc1341-7.2.1].
    /// Simply pass the value of the `boundary` key from the `Content-Type` header in the
    /// request (or use `Multipart::from_request()`, if supported).
    ///
    /// [rfc1341-7.2.1]: https://tools.ietf.org/html/rfc1341#page-30
    pub fn with_body<Bnd: Into<String>>(body: R, boundary: Bnd) -> Self {
        let boundary = boundary.into();

        info!("Multipart::with_boundary(_, {:?})", boundary);

        Multipart { 
            reader: BoundaryReader::from_reader(body, boundary),
        }
    }

    /// Read the next entry from this multipart request, returning a struct with the field's name and
    /// data. See `MultipartField` for more info.
    ///
    /// ## Warning: Risk of Data Loss
    /// If the previously returned entry had contents of type `MultipartField::File`,
    /// calling this again will discard any unread contents of that entry.
    pub fn read_entry(&mut self) -> io::Result<Option<MultipartField<&mut Self>>> {
        self.read_entry_mut().into_result()
    }

    /// Read the next entry from this multipart request, returning a struct with the field's name and
    /// data. See `MultipartField` for more info.
    pub fn into_entry(self) -> ReadEntryResult<Self> {
        self.read_entry()
    }

    /// Call `f` for each entry in the multipart request.
    /// 
    /// This is a substitute for Rust not supporting streaming iterators (where the return value
    /// from `next()` borrows the iterator for a bound lifetime).
    ///
    /// Returns `Ok(())` when all fields have been read, or the first error.
    pub fn foreach_entry<F>(&mut self, mut foreach: F) -> io::Result<()> where F: FnMut(MultipartField<&mut Self>) {
        loop {
            match self.read_entry() {
                Ok(Some(field)) => foreach(field),
                Ok(None) => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    }

    /// Get a builder type for saving the files in this request to the filesystem.
    ///
    /// See [`SaveBuilder`](save/struct.SaveBuilder.html) for more information.
    pub fn save(&mut self) -> SaveBuilder<&mut Self> {
        SaveBuilder::new(self)
    }
}

impl<R> Borrow<R> for Multipart<R> {
    fn borrow(&self) -> &R {
        self.reader.borrow()
    }
}

impl<R: Read> PrivReadEntry for Multipart<R> {
    type Source = BoundaryReader<R>;

    fn source_mut(&mut self) -> &mut BoundaryReader<R> {
        &mut self.reader
    }

    fn set_min_buf_size(&mut self, min_buf_size: usize) {
        self.reader.set_min_buf_size(min_buf_size)
    }

    /// Consume the next boundary.
    /// Returns `true` if a field should follow this boundary, `false` otherwise.
    fn consume_boundary(&mut self) -> io::Result<bool> {
        debug!("Consume boundary!");
        self.reader.consume_boundary()
    }
}

/// A server-side HTTP request that may or may not be multipart.
///
/// May be implemented by mutable references if providing the request or body by-value is
/// undesirable.
pub trait HttpRequest {
    /// The body of this request.
    type Body: Read;
    /// Get the boundary string of this request if it is a POST request
    /// with the `Content-Type` header set to `multipart/form-data`.
    ///
    /// The boundary string should be supplied as an extra value of the `Content-Type` header, e.g.
    /// `Content-Type: multipart/form-data; boundary={boundary}`.
    fn multipart_boundary(&self) -> Option<&str>;

    /// Return the request body for reading.
    fn body(self) -> Self::Body;
}

#[test]
fn issue_104() {
    ::init_log();

    use std::io::Cursor;

    let body = "\
    POST /test.html HTTP/1.1\r\n\
    Host: example.org\r\n\
    Content-Type: multipart/form-data;boundary=\"boundary\"\r\n\r\n\
    Content-Disposition: form-data; name=\"field1\"\r\n\r\n\
    value1\r\n\
    Content-Disposition: form-data; name=\"field2\"; filename=\"example.txt\"\r\n\r\n\
    value2 ";

    let request = Cursor::new(body);

    let mut multipart = Multipart::with_body(request, "boundary");
    multipart.foreach_entry(|_field| {/* Do nothing */}).unwrap_err();
}

#[test]
fn issue_114() {
    ::init_log();

    fn consume_all<R: BufRead>(mut rdr: R) {
        loop {
            let consume = rdr.fill_buf().unwrap().len();
            if consume == 0 { return; }
            rdr.consume(consume);
        }
    }

    use std::io::Cursor;

    let body = "\
    --------------------------c616e5fded96a3c7\r\n\
    Content-Disposition: form-data; name=\"key1\"\r\n\r\n\
    v1,\r\n\
    --------------------------c616e5fded96a3c7\r\n\
    Content-Disposition: form-data; name=\"key2\"\r\n\r\n\
    v2,\r\n\
    --------------------------c616e5fded96a3c7\r\n\
    Content-Disposition: form-data; name=\"key3\"\r\n\r\n\
    v3\r\n\
    --------------------------c616e5fded96a3c7--\r\n";

    let request = Cursor::new(body);
    let mut multipart = Multipart::with_body(request, "------------------------c616e5fded96a3c7");

    // one error if you do nothing
    multipart.foreach_entry(|_entry| { /* do nothing */}).unwrap();

    // a different error if you skip the first field
    multipart.foreach_entry(|entry| if *entry.headers.name != *"key1" { consume_all(entry.data); })
        .unwrap();


    multipart.foreach_entry(|_entry| () /* match entry.headers.name.as_str() {
        "file" => {
            let mut vec = Vec::new();
            entry.data.read_to_end(&mut vec).expect("can't read");
            // message.file = String::from_utf8(vec).ok();
            println!("key file got");
        }

        "key1" => {
            let mut vec = Vec::new();
            entry.data.read_to_end(&mut vec).expect("can't read");
            // message.key1 = String::from_utf8(vec).ok();
            println!("key1 got");
        }

        "key2" => {
            let mut vec = Vec::new();
            entry.data.read_to_end(&mut vec).expect("can't read");
            // message.key2 = String::from_utf8(vec).ok();
            println!("key2 got");
        }

        _ => {
            // as multipart has a bug https://github.com/abonander/multipart/issues/114
            // we manually do read_to_end here
            //let mut _vec = Vec::new();
            //entry.data.read_to_end(&mut _vec).expect("can't read");
            println!("key neglected");
        }
    }*/)
    .expect("Unable to iterate multipart?")
}
