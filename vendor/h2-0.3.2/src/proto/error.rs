use crate::codec::{RecvError, SendError};
use crate::frame::Reason;

use std::io;

/// Either an H2 reason  or an I/O error
#[derive(Debug)]
pub enum Error {
    Proto(Reason),
    Io(io::Error),
}

impl Error {
    /// Clone the error for internal purposes.
    ///
    /// `io::Error` is not `Clone`, so we only copy the `ErrorKind`.
    pub(super) fn shallow_clone(&self) -> Error {
        match *self {
            Error::Proto(reason) => Error::Proto(reason),
            Error::Io(ref io) => Error::Io(io::Error::from(io.kind())),
        }
    }
}

impl From<Reason> for Error {
    fn from(src: Reason) -> Self {
        Error::Proto(src)
    }
}

impl From<io::Error> for Error {
    fn from(src: io::Error) -> Self {
        Error::Io(src)
    }
}

impl From<Error> for RecvError {
    fn from(src: Error) -> RecvError {
        match src {
            Error::Proto(reason) => RecvError::Connection(reason),
            Error::Io(e) => RecvError::Io(e),
        }
    }
}

impl From<Error> for SendError {
    fn from(src: Error) -> SendError {
        match src {
            Error::Proto(reason) => SendError::Connection(reason),
            Error::Io(e) => SendError::Io(e),
        }
    }
}
