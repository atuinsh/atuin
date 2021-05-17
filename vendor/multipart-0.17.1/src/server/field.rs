// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! `multipart` field header parsing.
use mime::Mime;

use std::error::Error;
use std::io::{self, BufRead, Read};
use std::{fmt, str};

use std::sync::Arc;

use super::httparse::{self, Error as HttparseError, Header, Status, EMPTY_HEADER};

use self::ReadEntryResult::*;

use super::save::SaveBuilder;

const EMPTY_STR_HEADER: StrHeader<'static> = StrHeader { name: "", val: "" };

macro_rules! invalid_cont_disp {
    ($reason: expr, $cause: expr) => {
        return Err(ParseHeaderError::InvalidContDisp(
            $reason,
            $cause.to_string(),
        ));
    };
}

/// Not exposed
#[derive(Copy, Clone, Debug)]
pub struct StrHeader<'a> {
    name: &'a str,
    val: &'a str,
}

struct DisplayHeaders<'s, 'a: 's>(&'s [StrHeader<'a>]);

impl<'s, 'a: 's> fmt::Display for DisplayHeaders<'s, 'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for hdr in self.0 {
            writeln!(f, "{}: {}", hdr.name, hdr.val)?;
        }

        Ok(())
    }
}

fn with_headers<R, F, Ret>(r: &mut R, closure: F) -> Result<Ret, ParseHeaderError>
where
    R: BufRead,
    F: FnOnce(&[StrHeader]) -> Ret,
{
    const HEADER_LEN: usize = 4;

    let consume;
    let ret;

    let mut last_len = 0;

    loop {
        // this should return a larger buffer each time
        let buf = r.fill_buf()?;

        // buffer has stopped growing
        if buf.len() == last_len {
            return Err(ParseHeaderError::TooLarge);
        }

        let mut raw_headers = [EMPTY_HEADER; HEADER_LEN];

        match httparse::parse_headers(buf, &mut raw_headers)? {
            // read more and try again
            Status::Partial => last_len = buf.len(),
            Status::Complete((consume_, raw_headers)) => {
                let mut headers = [EMPTY_STR_HEADER; HEADER_LEN];
                let headers = copy_headers(raw_headers, &mut headers)?;
                debug!("Parsed headers: {:?}", headers);
                consume = consume_;
                ret = closure(headers);
                break;
            }
        }
    }

    r.consume(consume);
    Ok(ret)
}

fn copy_headers<'h, 'b: 'h>(
    raw: &[Header<'b>],
    headers: &'h mut [StrHeader<'b>],
) -> io::Result<&'h [StrHeader<'b>]> {
    for (raw, header) in raw.iter().zip(&mut *headers) {
        header.name = raw.name;
        header.val = io_str_utf8(raw.value)?;
    }

    Ok(&headers[..raw.len()])
}

/// The headers that (may) appear before a `multipart/form-data` field.
///
/// ### Warning: Values are Client-Provided
/// Everything in this struct are values from the client and should be considered **untrustworthy**.
/// This crate makes no effort to validate or sanitize any client inputs.
#[derive(Clone, Debug)]
pub struct FieldHeaders {
    /// The field's name from the form.
    pub name: Arc<str>,

    /// The filename of this entry, if supplied. This is not guaranteed to match the original file
    /// or even to be a valid filename for the current platform.
    pub filename: Option<String>,

    /// The MIME type (`Content-Type` value) of this file, if supplied by the client.
    ///
    /// If this is not supplied, the content-type of the field should default to `text/plain` as
    /// per [IETF RFC 7578, section 4.4](https://tools.ietf.org/html/rfc7578#section-4.4), but this
    /// should not be implicitly trusted. This crate makes no attempt to identify or validate
    /// the content-type of the actual field data.
    pub content_type: Option<Mime>,
}

impl FieldHeaders {
    /// Parse the field headers from the passed `BufRead`, consuming the relevant bytes.
    fn read_from<R: BufRead>(r: &mut R) -> Result<Self, ParseHeaderError> {
        with_headers(r, Self::parse)?
    }

    fn parse(headers: &[StrHeader]) -> Result<FieldHeaders, ParseHeaderError> {
        let cont_disp = ContentDisp::parse_required(headers)?;

        Ok(FieldHeaders {
            name: cont_disp.field_name.into(),
            filename: cont_disp.filename,
            content_type: parse_content_type(headers)?,
        })
    }
}

/// The `Content-Disposition` header.
struct ContentDisp {
    /// The name of the `multipart/form-data` field.
    field_name: String,
    /// The optional filename for this field.
    filename: Option<String>,
}

impl ContentDisp {
    fn parse_required(headers: &[StrHeader]) -> Result<ContentDisp, ParseHeaderError> {
        let header = if let Some(header) = find_header(headers, "Content-Disposition") {
            header
        } else {
            return Err(ParseHeaderError::MissingContentDisposition(
                DisplayHeaders(headers).to_string(),
            ));
        };

        // Content-Disposition: ?
        let after_disp_type = match split_once(header.val, ';') {
            Some((disp_type, after_disp_type)) => {
                // assert Content-Disposition: form-data
                // but needs to be parsed out to trim the spaces (allowed by spec IIRC)
                if disp_type.trim() != "form-data" {
                    invalid_cont_disp!("unexpected Content-Disposition value", disp_type);
                }
                after_disp_type
            }
            None => invalid_cont_disp!(
                "expected additional data after Content-Disposition type",
                header.val
            ),
        };

        // Content-Disposition: form-data; name=?
        let (field_name, filename) = match get_str_after("name=", ';', after_disp_type) {
            None => invalid_cont_disp!(
                "expected field name and maybe filename, got",
                after_disp_type
            ),
            // Content-Disposition: form-data; name={field_name}; filename=?
            Some((field_name, after_field_name)) => {
                let field_name = trim_quotes(field_name);
                let filename = get_str_after("filename=", ';', after_field_name)
                    .map(|(filename, _)| trim_quotes(filename).to_owned());
                (field_name, filename)
            }
        };

        Ok(ContentDisp {
            field_name: field_name.to_owned(),
            filename,
        })
    }
}

fn parse_content_type(headers: &[StrHeader]) -> Result<Option<Mime>, ParseHeaderError> {
    if let Some(header) = find_header(headers, "Content-Type") {
        // Boundary parameter will be parsed into the `Mime`
        debug!("Found Content-Type: {:?}", header.val);
        Ok(Some(header.val.parse::<Mime>().map_err(|_| {
            ParseHeaderError::MimeError(header.val.into())
        })?))
    } else {
        Ok(None)
    }
}

/// A field in a multipart request with its associated headers and data.
#[derive(Debug)]
pub struct MultipartField<M: ReadEntry> {
    /// The headers for this field, including the name, filename, and content-type, if provided.
    ///
    /// ### Warning: Values are Client-Provided
    /// Everything in this struct are values from the client and should be considered **untrustworthy**.
    /// This crate makes no effort to validate or sanitize any client inputs.
    pub headers: FieldHeaders,

    /// The field's data.
    pub data: MultipartData<M>,
}

impl<M: ReadEntry> MultipartField<M> {
    /// Returns `true` if this field has no content-type or the content-type is `text/...`.
    ///
    /// This typically means it can be read to a string, but it could still be using an unsupported
    /// character encoding, so decoding to `String` needs to ensure that the data is valid UTF-8.
    ///
    /// Note also that the field contents may be too large to reasonably fit in memory.
    /// The `.save()` adapter can be used to enforce a size limit.
    ///
    /// Detecting character encodings by any means is (currently) beyond the scope of this crate.
    pub fn is_text(&self) -> bool {
        self.headers
            .content_type
            .as_ref()
            .map_or(true, |ct| ct.type_() == mime::TEXT)
    }

    /// Read the next entry in the request.
    pub fn next_entry(self) -> ReadEntryResult<M> {
        self.data.into_inner().read_entry()
    }

    /// Update `self` as the next entry.
    ///
    /// Returns `Ok(Some(self))` if another entry was read, `Ok(None)` if the end of the body was
    /// reached, and `Err(e)` for any errors that occur.
    pub fn next_entry_inplace(&mut self) -> io::Result<Option<&mut Self>>
    where
        for<'a> &'a mut M: ReadEntry,
    {
        let multipart = self.data.take_inner();

        match multipart.read_entry() {
            Entry(entry) => {
                *self = entry;
                Ok(Some(self))
            }
            End(multipart) => {
                self.data.give_inner(multipart);
                Ok(None)
            }
            Error(multipart, err) => {
                self.data.give_inner(multipart);
                Err(err)
            }
        }
    }
}

/// The data of a field in a `multipart/form-data` request.
///
/// You can read it to EOF, or use the `save()` adaptor to save it to disk/memory.
#[derive(Debug)]
pub struct MultipartData<M> {
    inner: Option<M>,
}

const DATA_INNER_ERR: &str = "MultipartFile::inner taken and not replaced; this is likely \
                              caused by a logic error in `multipart` or by resuming after \
                              a previously caught panic.\nPlease open an issue with the \
                              relevant backtrace and debug logs at \
                              https://github.com/abonander/multipart";

impl<M> MultipartData<M>
where
    M: ReadEntry,
{
    /// Get a builder type which can save the field with or without a size limit.
    pub fn save(&mut self) -> SaveBuilder<&mut Self> {
        SaveBuilder::new(self)
    }

    /// Take the inner `Multipart` or `&mut Multipart`
    pub fn into_inner(self) -> M {
        self.inner.expect(DATA_INNER_ERR)
    }

    /// Set the minimum buffer size that `BufRead::fill_buf(self)` will return
    /// until the end of the stream is reached. Set this as small as you can tolerate
    /// to minimize `read()` calls (`read()` won't be called again until the buffer
    /// is smaller than this).
    ///
    /// This value is reset between fields.
    pub fn set_min_buf_size(&mut self, min_buf_size: usize) {
        self.inner_mut().set_min_buf_size(min_buf_size)
    }

    fn inner_mut(&mut self) -> &mut M {
        self.inner.as_mut().expect(DATA_INNER_ERR)
    }

    fn take_inner(&mut self) -> M {
        self.inner.take().expect(DATA_INNER_ERR)
    }

    fn give_inner(&mut self, inner: M) {
        self.inner = Some(inner);
    }
}

impl<M: ReadEntry> Read for MultipartData<M> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner_mut().source_mut().read(buf)
    }
}

/// In this implementation, `fill_buf()` can return more data with each call.
///
/// Use `set_min_buf_size()` if you require a minimum buffer length.
impl<M: ReadEntry> BufRead for MultipartData<M> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner_mut().source_mut().fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner_mut().source_mut().consume(amt)
    }
}

fn split_once(s: &str, delim: char) -> Option<(&str, &str)> {
    s.find(delim).map(|idx| s.split_at(idx))
}

fn trim_quotes(s: &str) -> &str {
    s.trim_matches('"')
}

/// Get the string after `needle` in `haystack`, stopping before `end_val_delim`
fn get_str_after<'a>(
    needle: &str,
    end_val_delim: char,
    haystack: &'a str,
) -> Option<(&'a str, &'a str)> {
    let val_start_idx = try_opt!(haystack.find(needle)) + needle.len();
    let val_end_idx = haystack[val_start_idx..]
        .find(end_val_delim)
        .map_or(haystack.len(), |end_idx| end_idx + val_start_idx);
    Some((
        &haystack[val_start_idx..val_end_idx],
        &haystack[val_end_idx..],
    ))
}

fn io_str_utf8(buf: &[u8]) -> io::Result<&str> {
    str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn find_header<'a, 'b>(headers: &'a [StrHeader<'b>], name: &str) -> Option<&'a StrHeader<'b>> {
    // Field names are case insensitive and consist of ASCII characters
    // only (see https://tools.ietf.org/html/rfc822#section-3.2).
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
}

/// Common trait for `Multipart` and `&mut Multipart`
pub trait ReadEntry: PrivReadEntry + Sized {
    /// Attempt to read the next entry in the multipart stream.
    fn read_entry(mut self) -> ReadEntryResult<Self> {
        self.set_min_buf_size(super::boundary::MIN_BUF_SIZE);

        debug!("ReadEntry::read_entry()");

        if !try_read_entry!(self; self.consume_boundary()) {
            return End(self);
        }

        let field_headers: FieldHeaders = try_read_entry!(self; self.read_headers());

        if let Some(ct) = field_headers.content_type.as_ref() {
            if ct.type_() == mime::MULTIPART {
                // fields of this type are sent by (supposedly) no known clients
                // (https://tools.ietf.org/html/rfc7578#appendix-A) so I'd be fascinated
                // to hear about any in the wild
                info!(
                    "Found nested multipart field: {:?}:\r\n\
                     Please report this client's User-Agent and any other available details \
                     at https://github.com/abonander/multipart/issues/56",
                    field_headers
                );
            }
        }

        Entry(MultipartField {
            headers: field_headers,
            data: MultipartData { inner: Some(self) },
        })
    }

    /// Equivalent to `read_entry()` but takes `&mut self`
    fn read_entry_mut(&mut self) -> ReadEntryResult<&mut Self> {
        ReadEntry::read_entry(self)
    }
}

impl<T> ReadEntry for T where T: PrivReadEntry {}

/// Public trait but not re-exported.
pub trait PrivReadEntry {
    type Source: BufRead;

    fn source_mut(&mut self) -> &mut Self::Source;

    fn set_min_buf_size(&mut self, min_buf_size: usize);

    /// Consume the next boundary.
    /// Returns `true` if a field should follow, `false` otherwise.
    fn consume_boundary(&mut self) -> io::Result<bool>;

    fn read_headers(&mut self) -> Result<FieldHeaders, io::Error> {
        FieldHeaders::read_from(self.source_mut())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn read_to_string(&mut self) -> io::Result<String> {
        let mut buf = String::new();

        match self.source_mut().read_to_string(&mut buf) {
            Ok(_) => Ok(buf),
            Err(err) => Err(err),
        }
    }
}

impl<'a, M: ReadEntry> PrivReadEntry for &'a mut M {
    type Source = M::Source;

    fn source_mut(&mut self) -> &mut M::Source {
        (**self).source_mut()
    }

    fn set_min_buf_size(&mut self, min_buf_size: usize) {
        (**self).set_min_buf_size(min_buf_size)
    }

    fn consume_boundary(&mut self) -> io::Result<bool> {
        (**self).consume_boundary()
    }
}

/// Ternary result type returned by `ReadEntry::next_entry()`,
/// `Multipart::into_entry()` and `MultipartField::next_entry()`.
pub enum ReadEntryResult<M: ReadEntry, Entry = MultipartField<M>> {
    /// The next entry was found.
    Entry(Entry),
    /// No  more entries could be read.
    End(M),
    /// An error occurred.
    Error(M, io::Error),
}

impl<M: ReadEntry, Entry> ReadEntryResult<M, Entry> {
    /// Convert `self` into `Result<Option<Entry>>` as follows:
    ///
    /// * `Entry(entry) -> Ok(Some(entry))`
    /// * `End(_) -> Ok(None)`
    /// * `Error(_, err) -> Err(err)`
    pub fn into_result(self) -> io::Result<Option<Entry>> {
        match self {
            ReadEntryResult::Entry(entry) => Ok(Some(entry)),
            ReadEntryResult::End(_) => Ok(None),
            ReadEntryResult::Error(_, err) => Err(err),
        }
    }

    /// Attempt to unwrap `Entry`, panicking if this is `End` or `Error`.
    pub fn unwrap(self) -> Entry {
        self.expect_alt(
            "`ReadEntryResult::unwrap()` called on `End` value",
            "`ReadEntryResult::unwrap()` called on `Error` value: {:?}",
        )
    }

    /// Attempt to unwrap `Entry`, panicking if this is `End` or `Error`
    /// with the given message. Adds the error's message in the `Error` case.
    pub fn expect(self, msg: &str) -> Entry {
        self.expect_alt(msg, msg)
    }

    /// Attempt to unwrap `Entry`, panicking if this is `End` or `Error`.
    /// If this is `End`, panics with `end_msg`; if `Error`, panics with `err_msg`
    /// as well as the error's message.
    pub fn expect_alt(self, end_msg: &str, err_msg: &str) -> Entry {
        match self {
            Entry(entry) => entry,
            End(_) => panic!("{}", end_msg),
            Error(_, err) => panic!("{}: {:?}", err_msg, err),
        }
    }

    /// Attempt to unwrap as `Option<Entry>`, panicking in the `Error` case.
    pub fn unwrap_opt(self) -> Option<Entry> {
        self.expect_opt("`ReadEntryResult::unwrap_opt()` called on `Error` value")
    }

    /// Attempt to unwrap as `Option<Entry>`, panicking in the `Error` case
    /// with the given message as well as the error's message.
    pub fn expect_opt(self, msg: &str) -> Option<Entry> {
        match self {
            Entry(entry) => Some(entry),
            End(_) => None,
            Error(_, err) => panic!("{}: {:?}", msg, err),
        }
    }
}

const GENERIC_PARSE_ERR: &str = "an error occurred while parsing field headers";

quick_error! {
    #[derive(Debug)]
    enum ParseHeaderError {
        /// The `Content-Disposition` header was not found
        MissingContentDisposition(headers: String) {
            display(x) -> ("{}:\n{}", x.description(), headers)
            description("\"Content-Disposition\" header not found in field headers")
        }
        InvalidContDisp(reason: &'static str, cause: String) {
            display(x) -> ("{}: {}: {}", x.description(), reason, cause)
            description("invalid \"Content-Disposition\" header")
        }
        /// The header was found but could not be parsed
        TokenizeError(err: HttparseError) {
            description(GENERIC_PARSE_ERR)
            display(x) -> ("{}: {}", x.description(), err)
            cause(err)
            from()
        }
        MimeError(cont_type: String) {
            description("Failed to parse Content-Type")
            display(this) -> ("{}: {}", this.description(), cont_type)
        }
        TooLarge {
            description("field headers section ridiculously long or missing trailing CRLF-CRLF")
        }
        /// IO error
        Io(err: io::Error) {
            description("an io error occurred while parsing the headers")
            display(x) -> ("{}: {}", x.description(), err)
            cause(err)
            from()
        }
    }
}

#[test]
fn test_find_header() {
    let headers = [
        StrHeader {
            name: "Content-Type",
            val: "text/plain",
        },
        StrHeader {
            name: "Content-disposition",
            val: "form-data",
        },
        StrHeader {
            name: "content-transfer-encoding",
            val: "binary",
        },
    ];

    assert_eq!(
        find_header(&headers, "Content-Type").unwrap().val,
        "text/plain"
    );
    assert_eq!(
        find_header(&headers, "Content-Disposition").unwrap().val,
        "form-data"
    );
    assert_eq!(
        find_header(&headers, "Content-Transfer-Encoding")
            .unwrap()
            .val,
        "binary"
    );
}
