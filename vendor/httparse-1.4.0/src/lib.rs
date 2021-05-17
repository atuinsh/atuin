#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![cfg_attr(test, deny(warnings))]
// we can't upgrade while supporting Rust 1.3
#![allow(deprecated)]
#![cfg_attr(httparse_min_2018, allow(rust_2018_idioms))]

//! # httparse
//!
//! A push library for parsing HTTP/1.x requests and responses.
//!
//! The focus is on speed and safety. Unsafe code is used to keep parsing fast,
//! but unsafety is contained in a submodule, with invariants enforced. The
//! parsing internals use an `Iterator` instead of direct indexing, while
//! skipping bounds checks.
//!
//! With Rust 1.27.0 or later, support for SIMD is enabled automatically.
//! If building an executable to be run on multiple platforms, and thus
//! not passing `target_feature` or `target_cpu` flags to the compiler,
//! runtime detection can still detect SSE4.2 or AVX2 support to provide
//! massive wins.
//!
//! If compiling for a specific target, remembering to include
//! `-C target_cpu=native` allows the detection to become compile time checks,
//! making it *even* faster.
#[cfg(feature = "std")]
extern crate std as core;

use core::{fmt, result, str, slice};

use iter::Bytes;

mod iter;
#[macro_use] mod macros;
mod simd;

#[inline]
fn shrink<T>(slice: &mut &mut [T], len: usize) {
    debug_assert!(slice.len() >= len);
    let ptr = slice.as_mut_ptr();
    *slice = unsafe { slice::from_raw_parts_mut(ptr, len) };
}

/// Determines if byte is a token char.
///
/// > ```notrust
/// > token          = 1*tchar
/// >
/// > tchar          = "!" / "#" / "$" / "%" / "&" / "'" / "*"
/// >                / "+" / "-" / "." / "^" / "_" / "`" / "|" / "~"
/// >                / DIGIT / ALPHA
/// >                ; any VCHAR, except delimiters
/// > ```
#[inline]
fn is_token(b: u8) -> bool {
    b > 0x1F && b < 0x7F
}

// ASCII codes to accept URI string.
// i.e. A-Z a-z 0-9 !#$%&'*+-._();:@=,/?[]~^
// TODO: Make a stricter checking for URI string?
static URI_MAP: [bool; 256] = byte_map![
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
//  \0                            \n
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
//  commands
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
//  \w !  "  #  $  %  &  '  (  )  *  +  ,  -  .  /
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 0, 1,
//  0  1  2  3  4  5  6  7  8  9  :  ;  <  =  >  ?
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
//  @  A  B  C  D  E  F  G  H  I  J  K  L  M  N  O
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
//  P  Q  R  S  T  U  V  W  X  Y  Z  [  \  ]  ^  _
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
//  `  a  b  c  d  e  f  g  h  i  j  k  l  m  n  o
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0,
//  p  q  r  s  t  u  v  w  x  y  z  {  |  }  ~  del
//   ====== Extended ASCII (aka. obs-text) ======
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[inline]
fn is_uri_token(b: u8) -> bool {
    URI_MAP[b as usize]
}

static HEADER_NAME_MAP: [bool; 256] = byte_map![
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 0, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[inline]
fn is_header_name_token(b: u8) -> bool {
    HEADER_NAME_MAP[b as usize]
}

static HEADER_VALUE_MAP: [bool; 256] = byte_map![
    0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];


#[inline]
fn is_header_value_token(b: u8) -> bool {
    HEADER_VALUE_MAP[b as usize]
}

/// An error in parsing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Error {
    /// Invalid byte in header name.
    HeaderName,
    /// Invalid byte in header value.
    HeaderValue,
    /// Invalid byte in new line.
    NewLine,
    /// Invalid byte in Response status.
    Status,
    /// Invalid byte where token is required.
    Token,
    /// Parsed more headers than provided buffer can contain.
    TooManyHeaders,
    /// Invalid byte in HTTP version.
    Version,
}

impl Error {
    #[inline]
    fn description_str(&self) -> &'static str {
        match *self {
            Error::HeaderName => "invalid header name",
            Error::HeaderValue => "invalid header value",
            Error::NewLine => "invalid new line",
            Error::Status => "invalid response status",
            Error::Token => "invalid token",
            Error::TooManyHeaders => "too many headers",
            Error::Version => "invalid HTTP version",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.description_str())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {
    fn description(&self) -> &str {
        self.description_str()
    }
}

/// An error in parsing a chunk size.
// Note: Move this into the error enum once v2.0 is released.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidChunkSize;

impl fmt::Display for InvalidChunkSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("invalid chunk size")
    }
}

/// A Result of any parsing action.
///
/// If the input is invalid, an `Error` will be returned. Note that incomplete
/// data is not considered invalid, and so will not return an error, but rather
/// a `Ok(Status::Partial)`.
pub type Result<T> = result::Result<Status<T>, Error>;

/// The result of a successful parse pass.
///
/// `Complete` is used when the buffer contained the complete value.
/// `Partial` is used when parsing did not reach the end of the expected value,
/// but no invalid data was found.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Status<T> {
    /// The completed result.
    Complete(T),
    /// A partial result.
    Partial
}

impl<T> Status<T> {
    /// Convenience method to check if status is complete.
    #[inline]
    pub fn is_complete(&self) -> bool {
        match *self {
            Status::Complete(..) => true,
            Status::Partial => false
        }
    }

    /// Convenience method to check if status is partial.
    #[inline]
    pub fn is_partial(&self) -> bool {
        match *self {
            Status::Complete(..) => false,
            Status::Partial => true
        }
    }

    /// Convenience method to unwrap a Complete value. Panics if the status is
    /// `Partial`.
    #[inline]
    pub fn unwrap(self) -> T {
        match self {
            Status::Complete(t) => t,
            Status::Partial => panic!("Tried to unwrap Status::Partial")
        }
    }
}

/// Parser configuration.
#[derive(Clone, Debug, Default)]
pub struct ParserConfig {
    allow_spaces_after_header_name_in_responses: bool,
}

impl ParserConfig {
    /// Sets whether spaces should be allowed after header name.
    pub fn allow_spaces_after_header_name_in_responses(
        &mut self,
        value: bool,
    ) -> &mut Self {
        self.allow_spaces_after_header_name_in_responses = value;
        self
    }

    /// Parses a response with the given config.
    pub fn parse_response<'headers, 'buf>(
        &self,
        response: &mut Response<'headers, 'buf>,
        buf: &'buf [u8],
    ) -> Result<usize> {
        response.parse_with_config(buf, self)
    }
}

/// A parsed Request.
///
/// The optional values will be `None` if a parse was not complete, and did not
/// parse the associated property. This allows you to inspect the parts that
/// could be parsed, before reading more, in case you wish to exit early.
///
/// # Example
///
/// ```no_run
/// let buf = b"GET /404 HTTP/1.1\r\nHost:";
/// let mut headers = [httparse::EMPTY_HEADER; 16];
/// let mut req = httparse::Request::new(&mut headers);
/// let res = req.parse(buf).unwrap();
/// if res.is_partial() {
///     match req.path {
///         Some(ref path) => {
///             // check router for path.
///             // /404 doesn't exist? we could stop parsing
///         },
///         None => {
///             // must read more and parse again
///         }
///     }
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct Request<'headers, 'buf: 'headers> {
    /// The request method, such as `GET`.
    pub method: Option<&'buf str>,
    /// The request path, such as `/about-us`.
    pub path: Option<&'buf str>,
    /// The request version, such as `HTTP/1.1`.
    pub version: Option<u8>,
    /// The request headers.
    pub headers: &'headers mut [Header<'buf>]
}

impl<'h, 'b> Request<'h, 'b> {
    /// Creates a new Request, using a slice of headers you allocate.
    #[inline]
    pub fn new(headers: &'h mut [Header<'b>]) -> Request<'h, 'b> {
        Request {
            method: None,
            path: None,
            version: None,
            headers: headers,
        }
    }

    /// Try to parse a buffer of bytes into the Request.
    /// 
    /// Returns byte offset in `buf` to start of HTTP body.
    pub fn parse(&mut self, buf: &'b [u8]) -> Result<usize> {
        let orig_len = buf.len();
        let mut bytes = Bytes::new(buf);
        complete!(skip_empty_lines(&mut bytes));
        self.method = Some(complete!(parse_token(&mut bytes)));
        self.path = Some(complete!(parse_uri(&mut bytes)));
        self.version = Some(complete!(parse_version(&mut bytes)));
        newline!(bytes);

        let len = orig_len - bytes.len();
        let headers_len = complete!(parse_headers_iter(
            &mut self.headers,
            &mut bytes,
            &ParserConfig::default(),
        ));

        Ok(Status::Complete(len + headers_len))
    }
}

#[inline]
fn skip_empty_lines(bytes: &mut Bytes) -> Result<()> {
    loop {
        let b = bytes.peek();
        match b {
            Some(b'\r') => {
                // there's `\r`, so it's safe to bump 1 pos
                unsafe { bytes.bump() };
                expect!(bytes.next() == b'\n' => Err(Error::NewLine));
            },
            Some(b'\n') => {
                // there's `\n`, so it's safe to bump 1 pos
                unsafe { bytes.bump(); }
            },
            Some(..) => {
                bytes.slice();
                return Ok(Status::Complete(()));
            },
            None => return Ok(Status::Partial)
        }
    }
}

/// A parsed Response.
///
/// See `Request` docs for explanation of optional values.
#[derive(Debug, Eq, PartialEq)]
pub struct Response<'headers, 'buf: 'headers> {
    /// The response version, such as `HTTP/1.1`.
    pub version: Option<u8>,
    /// The response code, such as `200`.
    pub code: Option<u16>,
    /// The response reason-phrase, such as `OK`.
    ///
    /// Contains an empty string if the reason-phrase was missing or contained invalid characters.
    pub reason: Option<&'buf str>,
    /// The response headers.
    pub headers: &'headers mut [Header<'buf>]
}

impl<'h, 'b> Response<'h, 'b> {
    /// Creates a new `Response` using a slice of `Header`s you have allocated.
    #[inline]
    pub fn new(headers: &'h mut [Header<'b>]) -> Response<'h, 'b> {
        Response {
            version: None,
            code: None,
            reason: None,
            headers: headers,
        }
    }

    /// Try to parse a buffer of bytes into this `Response`.
    pub fn parse(&mut self, buf: &'b [u8]) -> Result<usize> {
        self.parse_with_config(buf, &ParserConfig::default())
    }

    fn parse_with_config(&mut self, buf: &'b [u8], config: &ParserConfig) -> Result<usize> {
        let orig_len = buf.len();
        let mut bytes = Bytes::new(buf);

        complete!(skip_empty_lines(&mut bytes));
        self.version = Some(complete!(parse_version(&mut bytes)));
        space!(bytes or Error::Version);
        self.code = Some(complete!(parse_code(&mut bytes)));

        // RFC7230 says there must be 'SP' and then reason-phrase, but admits
        // its only for legacy reasons. With the reason-phrase completely
        // optional (and preferred to be omitted) in HTTP2, we'll just
        // handle any response that doesn't include a reason-phrase, because
        // it's more lenient, and we don't care anyways.
        //
        // So, a SP means parse a reason-phrase.
        // A newline means go to headers.
        // Anything else we'll say is a malformed status.
        match next!(bytes) {
            b' ' => {
                bytes.slice();
                self.reason = Some(complete!(parse_reason(&mut bytes)));
            },
            b'\r' => {
                expect!(bytes.next() == b'\n' => Err(Error::Status));
                bytes.slice();
                self.reason = Some("");
            },
            b'\n' => self.reason = Some(""),
            _ => return Err(Error::Status),
        }


        let len = orig_len - bytes.len();
        let headers_len = complete!(parse_headers_iter(&mut self.headers, &mut bytes, config));
        Ok(Status::Complete(len + headers_len))
    }
}

/// Represents a parsed header.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Header<'a> {
    /// The name portion of a header.
    ///
    /// A header name must be valid ASCII-US, so it's safe to store as a `&str`.
    pub name: &'a str,
    /// The value portion of a header.
    ///
    /// While headers **should** be ASCII-US, the specification allows for
    /// values that may not be, and so the value is stored as bytes.
    pub value: &'a [u8],
}

/// An empty header, useful for constructing a `Header` array to pass in for
/// parsing.
///
/// # Example
///
/// ```
/// let headers = [httparse::EMPTY_HEADER; 64];
/// ```
pub const EMPTY_HEADER: Header<'static> = Header { name: "", value: b"" };

#[inline]
fn parse_version(bytes: &mut Bytes) -> Result<u8> {
    if let Some(mut eight) = bytes.next_8() {
        expect!(eight._0() => b'H' |? Err(Error::Version));
        expect!(eight._1() => b'T' |? Err(Error::Version));
        expect!(eight._2() => b'T' |? Err(Error::Version));
        expect!(eight._3() => b'P' |? Err(Error::Version));
        expect!(eight._4() => b'/' |? Err(Error::Version));
        expect!(eight._5() => b'1' |? Err(Error::Version));
        expect!(eight._6() => b'.' |? Err(Error::Version));
        let v = match eight._7() {
            b'0' => 0,
            b'1' => 1,
            _ => return Err(Error::Version)
        };
        return Ok(Status::Complete(v))
    }

    // else (but not in `else` because of borrow checker)

    // If there aren't at least 8 bytes, we still want to detect early
    // if this is a valid version or not. If it is, we'll return Partial.
    expect!(bytes.next() == b'H' => Err(Error::Version));
    expect!(bytes.next() == b'T' => Err(Error::Version));
    expect!(bytes.next() == b'T' => Err(Error::Version));
    expect!(bytes.next() == b'P' => Err(Error::Version));
    expect!(bytes.next() == b'/' => Err(Error::Version));
    expect!(bytes.next() == b'1' => Err(Error::Version));
    expect!(bytes.next() == b'.' => Err(Error::Version));
    Ok(Status::Partial)
}

/// From [RFC 7230](https://tools.ietf.org/html/rfc7230):
///
/// > ```notrust
/// > reason-phrase  = *( HTAB / SP / VCHAR / obs-text )
/// > HTAB           = %x09        ; horizontal tab
/// > VCHAR          = %x21-7E     ; visible (printing) characters
/// > obs-text       = %x80-FF
/// > ```
///
/// > A.2.  Changes from RFC 2616
/// >
/// > Non-US-ASCII content in header fields and the reason phrase
/// > has been obsoleted and made opaque (the TEXT rule was removed).
#[inline]
fn parse_reason<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    let mut seen_obs_text = false;
    loop {
        let b = next!(bytes);
        if b == b'\r' {
            expect!(bytes.next() == b'\n' => Err(Error::Status));
            return Ok(Status::Complete(unsafe {
                let bytes = bytes.slice_skip(2);
                if !seen_obs_text {
                    // all bytes up till `i` must have been HTAB / SP / VCHAR
                    str::from_utf8_unchecked(bytes)
                } else {
                    // obs-text characters were found, so return the fallback empty string
                    ""
                }
            }));
        } else if b == b'\n' {
            return Ok(Status::Complete(unsafe {
                let bytes = bytes.slice_skip(1);
                if !seen_obs_text {
                    // all bytes up till `i` must have been HTAB / SP / VCHAR
                    str::from_utf8_unchecked(bytes)
                } else {
                    // obs-text characters were found, so return the fallback empty string
                    ""
                }
            }));
        } else if !(b == 0x09 || b == b' ' || (b >= 0x21 && b <= 0x7E) || b >= 0x80) {
            return Err(Error::Status);
        } else if b >= 0x80 {
            seen_obs_text = true;
        }
    }
}

#[inline]
fn parse_token<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    loop {
        let b = next!(bytes);
        if b == b' ' {
            return Ok(Status::Complete(unsafe {
                // all bytes up till `i` must have been `is_token`.
                str::from_utf8_unchecked(bytes.slice_skip(1))
            }));
        } else if !is_token(b) {
            return Err(Error::Token);
        }
    }
}

#[inline]
fn parse_uri<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    simd::match_uri_vectored(bytes);

    loop {
        let b = next!(bytes);
        if b == b' ' {
            return Ok(Status::Complete(unsafe {
                // all bytes up till `i` must have been `is_token`.
                str::from_utf8_unchecked(bytes.slice_skip(1))
            }));
        } else if !is_uri_token(b) {
            return Err(Error::Token);
        }
    }
}


#[inline]
fn parse_code(bytes: &mut Bytes) -> Result<u16> {
    let hundreds = expect!(bytes.next() == b'0'...b'9' => Err(Error::Status));
    let tens = expect!(bytes.next() == b'0'...b'9' => Err(Error::Status));
    let ones = expect!(bytes.next() == b'0'...b'9' => Err(Error::Status));

    Ok(Status::Complete((hundreds - b'0') as u16 * 100 +
        (tens - b'0') as u16 * 10 +
        (ones - b'0') as u16))
}

/// Parse a buffer of bytes as headers.
///
/// The return value, if complete and successful, includes the index of the
/// buffer that parsing stopped at, and a sliced reference to the parsed
/// headers. The length of the slice will be equal to the number of properly
/// parsed headers.
///
/// # Example
///
/// ```
/// let buf = b"Host: foo.bar\nAccept: */*\n\nblah blah";
/// let mut headers = [httparse::EMPTY_HEADER; 4];
/// assert_eq!(httparse::parse_headers(buf, &mut headers),
///            Ok(httparse::Status::Complete((27, &[
///                httparse::Header { name: "Host", value: b"foo.bar" },
///                httparse::Header { name: "Accept", value: b"*/*" }
///            ][..]))));
/// ```
pub fn parse_headers<'b: 'h, 'h>(
    src: &'b [u8],
    mut dst: &'h mut [Header<'b>],
) -> Result<(usize, &'h [Header<'b>])> {
    let mut iter = Bytes::new(src);
    let pos = complete!(parse_headers_iter(&mut dst, &mut iter, &ParserConfig::default()));
    Ok(Status::Complete((pos, dst)))
}

#[inline]
fn parse_headers_iter<'a, 'b>(
    headers: &mut &mut [Header<'a>],
    bytes: &'b mut Bytes<'a>,
    config: &ParserConfig,
) -> Result<usize> {
    let mut num_headers: usize = 0;
    let mut count: usize = 0;
    let mut result = Err(Error::TooManyHeaders);

    {
        let mut iter = headers.iter_mut();

        'headers: loop {
            // a newline here means the head is over!
            let b = next!(bytes);
            if b == b'\r' {
                expect!(bytes.next() == b'\n' => Err(Error::NewLine));
                result = Ok(Status::Complete(count + bytes.pos()));
                break;
            } else if b == b'\n' {
                result = Ok(Status::Complete(count + bytes.pos()));
                break;
            } else if !is_header_name_token(b) {
                return Err(Error::HeaderName);
            }

            let header = match iter.next() {
                Some(header) => header,
                None => break 'headers
            };

            num_headers += 1;
            // parse header name until colon
            'name: loop {
                let b = next!(bytes);
                if b == b':' {
                    count += bytes.pos();
                    header.name = unsafe {
                        str::from_utf8_unchecked(bytes.slice_skip(1))
                    };
                    break 'name;
                } else if !is_header_name_token(b) {
                    if !config.allow_spaces_after_header_name_in_responses {
                        return Err(Error::HeaderName);
                    }
                    count += bytes.pos();
                    header.name = unsafe { str::from_utf8_unchecked(bytes.slice_skip(1)) };
                    // eat white space between name and colon
                    'whitespace_before_colon: loop {
                        let b = next!(bytes);
                        if b == b' ' || b == b'\t' {
                            count += bytes.pos();
                            bytes.slice();
                            continue 'whitespace_before_colon;
                        } else if b == b':' {
                            count += bytes.pos();
                            bytes.slice();
                            break 'name;
                        } else {
                            return Err(Error::HeaderName);
                        }
                    }
                }
            }

            let mut b;

            'value: loop {

                // eat white space between colon and value
                'whitespace_after_colon: loop {
                    b = next!(bytes);
                    if b == b' ' || b == b'\t' {
                        count += bytes.pos();
                        bytes.slice();
                        continue 'whitespace_after_colon;
                    } else {
                        if !is_header_value_token(b) {
                            break 'value;
                        }
                        break 'whitespace_after_colon;
                    }
                }

                // parse value till EOL

                simd::match_header_value_vectored(bytes);

                macro_rules! check {
                    ($bytes:ident, $i:ident) => ({
                        b = $bytes.$i();
                        if !is_header_value_token(b) {
                            break 'value;
                        }
                    });
                    ($bytes:ident) => ({
                        check!($bytes, _0);
                        check!($bytes, _1);
                        check!($bytes, _2);
                        check!($bytes, _3);
                        check!($bytes, _4);
                        check!($bytes, _5);
                        check!($bytes, _6);
                        check!($bytes, _7);
                    })
                }
                while let Some(mut bytes8) = bytes.next_8() {
                    check!(bytes8);
                }
                loop {
                    b = next!(bytes);
                    if !is_header_value_token(b) {
                        break 'value;
                    }
                }
            }

            //found_ctl
            let value_slice : &[u8] = if b == b'\r' {
                expect!(bytes.next() == b'\n' => Err(Error::HeaderValue));
                count += bytes.pos();
                // having just check that `\r\n` exists, it's safe to skip those 2 bytes
                unsafe {
                    bytes.slice_skip(2)
                }
            } else if b == b'\n' {
                count += bytes.pos();
                // having just check that `\r\n` exists, it's safe to skip 1 byte
                unsafe {
                    bytes.slice_skip(1)
                }
            } else {
                return Err(Error::HeaderValue);
            };
            // trim trailing whitespace in the header
            if let Some(last_visible) = value_slice.iter().rposition(|b| *b != b' ' && *b != b'\t' ) {
                // There is at least one non-whitespace character.
                header.value = &value_slice[0..last_visible+1];
            } else {
                // There is no non-whitespace character. This can only happen when value_slice is
                // empty.
                header.value = value_slice;
            }
        }
    } // drop iter

    shrink(headers, num_headers);
    result
}

/// Parse a buffer of bytes as a chunk size.
///
/// The return value, if complete and successful, includes the index of the
/// buffer that parsing stopped at, and the size of the following chunk.
///
/// # Example
///
/// ```
/// let buf = b"4\r\nRust\r\n0\r\n\r\n";
/// assert_eq!(httparse::parse_chunk_size(buf),
///            Ok(httparse::Status::Complete((3, 4))));
/// ```
pub fn parse_chunk_size(buf: &[u8])
    -> result::Result<Status<(usize, u64)>, InvalidChunkSize> {
    const RADIX: u64 = 16;
    let mut bytes = Bytes::new(buf);
    let mut size = 0;
    let mut in_chunk_size = true;
    let mut in_ext = false;
    let mut count = 0;
    loop {
        let b = next!(bytes);
        match b {
            b'0' ... b'9' if in_chunk_size => {
                if count > 15 {
                    return Err(InvalidChunkSize);
                }
                count += 1;
                size *= RADIX;
                size += (b - b'0') as u64;
            },
            b'a' ... b'f' if in_chunk_size => {
                if count > 15 {
                    return Err(InvalidChunkSize);
                }
                count += 1;
                size *= RADIX;
                size += (b + 10 - b'a') as u64;
            }
            b'A' ... b'F' if in_chunk_size => {
                if count > 15 {
                    return Err(InvalidChunkSize);
                }
                count += 1;
                size *= RADIX;
                size += (b + 10 - b'A') as u64;
            }
            b'\r' => {
                match next!(bytes) {
                    b'\n' => break,
                    _ => return Err(InvalidChunkSize),
                }
            }
            // If we weren't in the extension yet, the ";" signals its start
            b';' if !in_ext => {
                in_ext = true;
                in_chunk_size = false;
            }
            // "Linear white space" is ignored between the chunk size and the
            // extension separator token (";") due to the "implied *LWS rule".
            b'\t' | b' ' if !in_ext && !in_chunk_size => {}
            // LWS can follow the chunk size, but no more digits can come
            b'\t' | b' ' if in_chunk_size => in_chunk_size = false,
            // We allow any arbitrary octet once we are in the extension, since
            // they all get ignored anyway. According to the HTTP spec, valid
            // extensions would have a more strict syntax:
            //     (token ["=" (token | quoted-string)])
            // but we gain nothing by rejecting an otherwise valid chunk size.
            _ if in_ext => {}
            // Finally, if we aren't in the extension and we're reading any
            // other octet, the chunk size line is invalid!
            _ => return Err(InvalidChunkSize),
        }
    }
    Ok(Status::Complete((bytes.pos(), size)))
}

#[cfg(test)]
mod tests {
    use super::{Request, Response, Status, EMPTY_HEADER, shrink, parse_chunk_size};

    const NUM_OF_HEADERS: usize = 4;

    #[test]
    fn test_shrink() {
        let mut arr = [EMPTY_HEADER; 16];
        {
            let slice = &mut &mut arr[..];
            assert_eq!(slice.len(), 16);
            shrink(slice, 4);
            assert_eq!(slice.len(), 4);
        }
        assert_eq!(arr.len(), 16);
    }

    macro_rules! req {
        ($name:ident, $buf:expr, |$arg:ident| $body:expr) => (
            req! {$name, $buf, Ok(Status::Complete($buf.len())), |$arg| $body }
        );
        ($name:ident, $buf:expr, $len:expr, |$arg:ident| $body:expr) => (
        #[test]
        fn $name() {
            let mut headers = [EMPTY_HEADER; NUM_OF_HEADERS];
            let mut req = Request::new(&mut headers[..]);
            let status = req.parse($buf.as_ref());
            assert_eq!(status, $len);
            closure(req);

            fn closure($arg: Request) {
                $body
            }
        }
        )
    }

    req! {
        test_request_simple,
        b"GET / HTTP/1.1\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_simple_with_query_params,
        b"GET /thing?data=a HTTP/1.1\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/thing?data=a");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_simple_with_whatwg_query_params,
        b"GET /thing?data=a^ HTTP/1.1\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/thing?data=a^");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_headers,
        b"GET / HTTP/1.1\r\nHost: foo.com\r\nCookie: \r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 2);
            assert_eq!(req.headers[0].name, "Host");
            assert_eq!(req.headers[0].value, b"foo.com");
            assert_eq!(req.headers[1].name, "Cookie");
            assert_eq!(req.headers[1].value, b"");
        }
    }

    req! {
        test_request_headers_optional_whitespace,
        b"GET / HTTP/1.1\r\nHost: \tfoo.com\t \r\nCookie: \t \r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 2);
            assert_eq!(req.headers[0].name, "Host");
            assert_eq!(req.headers[0].value, b"foo.com");
            assert_eq!(req.headers[1].name, "Cookie");
            assert_eq!(req.headers[1].value, b"");
        }
    }

    req! {
        // test the scalar parsing
        test_request_header_value_htab_short,
        b"GET / HTTP/1.1\r\nUser-Agent: some\tagent\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 1);
            assert_eq!(req.headers[0].name, "User-Agent");
            assert_eq!(req.headers[0].value, b"some\tagent");
        }
    }

    req! {
        // test the sse42 parsing
        test_request_header_value_htab_med,
        b"GET / HTTP/1.1\r\nUser-Agent: 1234567890some\tagent\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 1);
            assert_eq!(req.headers[0].name, "User-Agent");
            assert_eq!(req.headers[0].value, b"1234567890some\tagent");
        }
    }

    req! {
        // test the avx2 parsing
        test_request_header_value_htab_long,
        b"GET / HTTP/1.1\r\nUser-Agent: 1234567890some\t1234567890agent1234567890\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 1);
            assert_eq!(req.headers[0].name, "User-Agent");
            assert_eq!(req.headers[0].value, &b"1234567890some\t1234567890agent1234567890"[..]);
        }
    }

    req! {
        test_request_headers_max,
        b"GET / HTTP/1.1\r\nA: A\r\nB: B\r\nC: C\r\nD: D\r\n\r\n",
        |req| {
            assert_eq!(req.headers.len(), NUM_OF_HEADERS);
        }
    }

    req! {
        test_request_multibyte,
        b"GET / HTTP/1.1\r\nHost: foo.com\r\nUser-Agent: \xe3\x81\xb2\xe3/1.0\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers[0].name, "Host");
            assert_eq!(req.headers[0].value, b"foo.com");
            assert_eq!(req.headers[1].name, "User-Agent");
            assert_eq!(req.headers[1].value, b"\xe3\x81\xb2\xe3/1.0");
        }
    }


    req! {
        test_request_partial,
        b"GET / HTTP/1.1\r\n\r", Ok(Status::Partial),
        |_req| {}
    }

    req! {
        test_request_partial_version,
        b"GET / HTTP/1.", Ok(Status::Partial),
        |_req| {}
    }

    req! {
        test_request_newlines,
        b"GET / HTTP/1.1\nHost: foo.bar\n\n",
        |_r| {}
    }

    req! {
        test_request_empty_lines_prefix,
        b"\r\n\r\nGET / HTTP/1.1\r\n\r\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_empty_lines_prefix_lf_only,
        b"\n\nGET / HTTP/1.1\n\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_path_backslash,
        b"\n\nGET /\\?wayne\\=5 HTTP/1.1\n\n",
        |req| {
            assert_eq!(req.method.unwrap(), "GET");
            assert_eq!(req.path.unwrap(), "/\\?wayne\\=5");
            assert_eq!(req.version.unwrap(), 1);
            assert_eq!(req.headers.len(), 0);
        }
    }

    req! {
        test_request_with_invalid_token_delimiter,
        b"GET\n/ HTTP/1.1\r\nHost: foo.bar\r\n\r\n",
        Err(::Error::Token),
        |_r| {}
    }


    req! {
        test_request_with_invalid_but_short_version,
        b"GET / HTTP/1!",
        Err(::Error::Version),
        |_r| {}
    }

    macro_rules! res {
        ($name:ident, $buf:expr, |$arg:ident| $body:expr) => (
            res! {$name, $buf, Ok(Status::Complete($buf.len())), |$arg| $body }
        );
        ($name:ident, $buf:expr, $len:expr, |$arg:ident| $body:expr) => (
        #[test]
        fn $name() {
            let mut headers = [EMPTY_HEADER; NUM_OF_HEADERS];
            let mut res = Response::new(&mut headers[..]);
            let status = res.parse($buf.as_ref());
            assert_eq!(status, $len);
            closure(res);

            fn closure($arg: Response) {
                $body
            }
        }
        )
    }

    res! {
        test_response_simple,
        b"HTTP/1.1 200 OK\r\n\r\n",
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 200);
            assert_eq!(res.reason.unwrap(), "OK");
        }
    }

    res! {
        test_response_newlines,
        b"HTTP/1.0 403 Forbidden\nServer: foo.bar\n\n",
        |_r| {}
    }

    res! {
        test_response_reason_missing,
        b"HTTP/1.1 200 \r\n\r\n",
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 200);
            assert_eq!(res.reason.unwrap(), "");
        }
    }

    res! {
        test_response_reason_missing_no_space,
        b"HTTP/1.1 200\r\n\r\n",
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 200);
            assert_eq!(res.reason.unwrap(), "");
        }
    }

    res! {
        test_response_reason_missing_no_space_with_headers,
        b"HTTP/1.1 200\r\nFoo: bar\r\n\r\n",
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 200);
            assert_eq!(res.reason.unwrap(), "");
            assert_eq!(res.headers.len(), 1);
            assert_eq!(res.headers[0].name, "Foo");
            assert_eq!(res.headers[0].value, b"bar");
        }
    }

    res! {
        test_response_reason_with_space_and_tab,
        b"HTTP/1.1 101 Switching Protocols\t\r\n\r\n",
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 101);
            assert_eq!(res.reason.unwrap(), "Switching Protocols\t");
        }
    }

    static RESPONSE_REASON_WITH_OBS_TEXT_BYTE: &'static [u8] = b"HTTP/1.1 200 X\xFFZ\r\n\r\n";
    res! {
        test_response_reason_with_obsolete_text_byte,
        RESPONSE_REASON_WITH_OBS_TEXT_BYTE,
        |res| {
            assert_eq!(res.version.unwrap(), 1);
            assert_eq!(res.code.unwrap(), 200);
            // Empty string fallback in case of obs-text
            assert_eq!(res.reason.unwrap(), "");
        }
    }

    res! {
        test_response_reason_with_nul_byte,
        b"HTTP/1.1 200 \x00\r\n\r\n",
        Err(::Error::Status),
        |_res| {}
    }

    res! {
        test_response_version_missing_space,
        b"HTTP/1.1",
        Ok(Status::Partial),
        |_res| {}
    }

    res! {
        test_response_code_missing_space,
        b"HTTP/1.1 200",
        Ok(Status::Partial),
        |_res| {}
    }

    res! {
        test_response_empty_lines_prefix_lf_only,
        b"\n\nHTTP/1.1 200 OK\n\n",
        |_res| {}
    }

    static RESPONSE_WITH_WHITESPACE_BETWEEN_HEADER_NAME_AND_COLON: &'static [u8] =
        b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Credentials : true\r\n\r\n";

    #[test]
    fn test_forbid_response_with_whitespace_between_header_name_and_colon() {
        let mut headers = [EMPTY_HEADER; 1];
        let mut response = Response::new(&mut headers[..]);
        let result = response.parse(RESPONSE_WITH_WHITESPACE_BETWEEN_HEADER_NAME_AND_COLON);

        assert_eq!(result, Err(::Error::HeaderName));
    }

    #[test]
    fn test_allow_response_with_whitespace_between_header_name_and_colon() {
        let mut headers = [EMPTY_HEADER; 1];
        let mut response = Response::new(&mut headers[..]);
        let result = ::ParserConfig::default()
            .allow_spaces_after_header_name_in_responses(true)
            .parse_response(&mut response, RESPONSE_WITH_WHITESPACE_BETWEEN_HEADER_NAME_AND_COLON);

        assert_eq!(result, Ok(Status::Complete(60)));
        assert_eq!(response.version.unwrap(), 1);
        assert_eq!(response.code.unwrap(), 200);
        assert_eq!(response.reason.unwrap(), "OK");
        assert_eq!(response.headers.len(), 1);
        assert_eq!(response.headers[0].name, "Access-Control-Allow-Credentials");
        assert_eq!(response.headers[0].value, &b"true"[..]);
    }

    static REQUEST_WITH_WHITESPACE_BETWEEN_HEADER_NAME_AND_COLON: &'static [u8] =
        b"GET / HTTP/1.1\r\nHost : localhost\r\n\r\n";

    #[test]
    fn test_forbid_request_with_whitespace_between_header_name_and_colon() {
        let mut headers = [EMPTY_HEADER; 1];
        let mut request = Request::new(&mut headers[..]);
        let result = request.parse(REQUEST_WITH_WHITESPACE_BETWEEN_HEADER_NAME_AND_COLON);

        assert_eq!(result, Err(::Error::HeaderName));
    }

    #[test]
    fn test_chunk_size() {
        assert_eq!(parse_chunk_size(b"0\r\n"), Ok(Status::Complete((3, 0))));
        assert_eq!(parse_chunk_size(b"12\r\nchunk"), Ok(Status::Complete((4, 18))));
        assert_eq!(parse_chunk_size(b"3086d\r\n"), Ok(Status::Complete((7, 198765))));
        assert_eq!(parse_chunk_size(b"3735AB1;foo bar*\r\n"), Ok(Status::Complete((18, 57891505))));
        assert_eq!(parse_chunk_size(b"3735ab1 ; baz \r\n"), Ok(Status::Complete((16, 57891505))));
        assert_eq!(parse_chunk_size(b"77a65\r"), Ok(Status::Partial));
        assert_eq!(parse_chunk_size(b"ab"), Ok(Status::Partial));
        assert_eq!(parse_chunk_size(b"567f8a\rfoo"), Err(::InvalidChunkSize));
        assert_eq!(parse_chunk_size(b"567f8a\rfoo"), Err(::InvalidChunkSize));
        assert_eq!(parse_chunk_size(b"567xf8a\r\n"), Err(::InvalidChunkSize));
        assert_eq!(parse_chunk_size(b"ffffffffffffffff\r\n"), Ok(Status::Complete((18, ::core::u64::MAX))));
        assert_eq!(parse_chunk_size(b"1ffffffffffffffff\r\n"), Err(::InvalidChunkSize));
        assert_eq!(parse_chunk_size(b"Affffffffffffffff\r\n"), Err(::InvalidChunkSize));
        assert_eq!(parse_chunk_size(b"fffffffffffffffff\r\n"), Err(::InvalidChunkSize));
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_std_error() {
        use super::Error;
        use std::error::Error as StdError;
        let err = Error::HeaderName;
        assert_eq!(err.to_string(), err.description());
    }
}
