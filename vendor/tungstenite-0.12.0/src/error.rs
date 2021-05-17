//! Error handling.

use std::{borrow::Cow, error::Error as ErrorTrait, fmt, io, result, str, string};

use crate::protocol::Message;
use http::Response;

#[cfg(feature = "tls")]
pub mod tls {
    //! TLS error wrapper module, feature-gated.
    pub use native_tls::Error;
}

/// Result type of all Tungstenite library calls.
pub type Result<T> = result::Result<T, Error>;

/// Possible WebSocket errors
#[derive(Debug)]
pub enum Error {
    /// WebSocket connection closed normally. This informs you of the close.
    /// It's not an error as such and nothing wrong happened.
    ///
    /// This is returned as soon as the close handshake is finished (we have both sent and
    /// received a close frame) on the server end and as soon as the server has closed the
    /// underlying connection if this endpoint is a client.
    ///
    /// Thus when you receive this, it is safe to drop the underlying connection.
    ///
    /// Receiving this error means that the WebSocket object is not usable anymore and the
    /// only meaningful action with it is dropping it.
    ConnectionClosed,
    /// Trying to work with already closed connection.
    ///
    /// Trying to read or write after receiving `ConnectionClosed` causes this.
    ///
    /// As opposed to `ConnectionClosed`, this indicates your code tries to operate on the
    /// connection when it really shouldn't anymore, so this really indicates a programmer
    /// error on your part.
    AlreadyClosed,
    /// Input-output error. Apart from WouldBlock, these are generally errors with the
    /// underlying connection and you should probably consider them fatal.
    Io(io::Error),
    #[cfg(feature = "tls")]
    /// TLS error
    Tls(tls::Error),
    /// - When reading: buffer capacity exhausted.
    /// - When writing: your message is bigger than the configured max message size
    ///   (64MB by default).
    Capacity(Cow<'static, str>),
    /// Protocol violation.
    Protocol(Cow<'static, str>),
    /// Message send queue full.
    SendQueueFull(Message),
    /// UTF coding error
    Utf8,
    /// Invalid URL.
    Url(Cow<'static, str>),
    /// HTTP error.
    Http(Response<Option<String>>),
    /// HTTP format error.
    HttpFormat(http::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ConnectionClosed => write!(f, "Connection closed normally"),
            Error::AlreadyClosed => write!(f, "Trying to work with closed connection"),
            Error::Io(ref err) => write!(f, "IO error: {}", err),
            #[cfg(feature = "tls")]
            Error::Tls(ref err) => write!(f, "TLS error: {}", err),
            Error::Capacity(ref msg) => write!(f, "Space limit exceeded: {}", msg),
            Error::Protocol(ref msg) => write!(f, "WebSocket protocol error: {}", msg),
            Error::SendQueueFull(_) => write!(f, "Send queue is full"),
            Error::Utf8 => write!(f, "UTF-8 encoding error"),
            Error::Url(ref msg) => write!(f, "URL error: {}", msg),
            Error::Http(ref code) => write!(f, "HTTP error: {}", code.status()),
            Error::HttpFormat(ref err) => write!(f, "HTTP format error: {}", err),
        }
    }
}

impl ErrorTrait for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<str::Utf8Error> for Error {
    fn from(_: str::Utf8Error) -> Self {
        Error::Utf8
    }
}

impl From<string::FromUtf8Error> for Error {
    fn from(_: string::FromUtf8Error) -> Self {
        Error::Utf8
    }
}

impl From<http::header::InvalidHeaderValue> for Error {
    fn from(err: http::header::InvalidHeaderValue) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::header::InvalidHeaderName> for Error {
    fn from(err: http::header::InvalidHeaderName) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::header::ToStrError> for Error {
    fn from(_: http::header::ToStrError) -> Self {
        Error::Utf8
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(err: http::uri::InvalidUri) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::status::InvalidStatusCode> for Error {
    fn from(err: http::status::InvalidStatusCode) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::Error> for Error {
    fn from(err: http::Error) -> Self {
        Error::HttpFormat(err)
    }
}

#[cfg(feature = "tls")]
impl From<tls::Error> for Error {
    fn from(err: tls::Error) -> Self {
        Error::Tls(err)
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        match err {
            httparse::Error::TooManyHeaders => Error::Capacity("Too many headers".into()),
            e => Error::Protocol(e.to_string().into()),
        }
    }
}
