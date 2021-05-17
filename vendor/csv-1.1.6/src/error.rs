use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::result;

use crate::byte_record::{ByteRecord, Position};
use crate::deserializer::DeserializeError;

/// A type alias for `Result<T, csv::Error>`.
pub type Result<T> = result::Result<T, Error>;

/// An error that can occur when processing CSV data.
///
/// This error can happen when writing or reading CSV data.
///
/// There are some important scenarios where an error is impossible to occur.
/// For example, if a CSV reader is used on an in-memory buffer with the
/// `flexible` option enabled and one is reading records as raw byte strings,
/// then no error can occur.
#[derive(Debug)]
pub struct Error(Box<ErrorKind>);

impl Error {
    /// A crate private constructor for `Error`.
    pub(crate) fn new(kind: ErrorKind) -> Error {
        Error(Box::new(kind))
    }

    /// Return the specific type of this error.
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    /// Unwrap this error into its underlying type.
    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }

    /// Returns true if this is an I/O error.
    ///
    /// If this is true, the underlying `ErrorKind` is guaranteed to be
    /// `ErrorKind::Io`.
    pub fn is_io_error(&self) -> bool {
        match *self.0 {
            ErrorKind::Io(_) => true,
            _ => false,
        }
    }

    /// Return the position for this error, if one exists.
    ///
    /// This is a convenience function that permits callers to easily access
    /// the position on an error without doing case analysis on `ErrorKind`.
    pub fn position(&self) -> Option<&Position> {
        self.0.position()
    }
}

/// The specific type of an error.
#[derive(Debug)]
pub enum ErrorKind {
    /// An I/O error that occurred while reading CSV data.
    Io(io::Error),
    /// A UTF-8 decoding error that occured while reading CSV data into Rust
    /// `String`s.
    Utf8 {
        /// The position of the record in which this error occurred, if
        /// available.
        pos: Option<Position>,
        /// The corresponding UTF-8 error.
        err: Utf8Error,
    },
    /// This error occurs when two records with an unequal number of fields
    /// are found. This error only occurs when the `flexible` option in a
    /// CSV reader/writer is disabled.
    UnequalLengths {
        /// The position of the first record with an unequal number of fields
        /// to the previous record, if available.
        pos: Option<Position>,
        /// The expected number of fields in a record. This is the number of
        /// fields in the record read prior to the record indicated by
        /// `pos`.
        expected_len: u64,
        /// The number of fields in the bad record.
        len: u64,
    },
    /// This error occurs when either the `byte_headers` or `headers` methods
    /// are called on a CSV reader that was asked to `seek` before it parsed
    /// the first record.
    Seek,
    /// An error of this kind occurs only when using the Serde serializer.
    Serialize(String),
    /// An error of this kind occurs only when performing automatic
    /// deserialization with serde.
    Deserialize {
        /// The position of this error, if available.
        pos: Option<Position>,
        /// The deserialization error.
        err: DeserializeError,
    },
    /// Hints that destructuring should not be exhaustive.
    ///
    /// This enum may grow additional variants, so this makes sure clients
    /// don't count on exhaustive matching. (Otherwise, adding a new variant
    /// could break existing code.)
    #[doc(hidden)]
    __Nonexhaustive,
}

impl ErrorKind {
    /// Return the position for this error, if one exists.
    ///
    /// This is a convenience function that permits callers to easily access
    /// the position on an error without doing case analysis on `ErrorKind`.
    pub fn position(&self) -> Option<&Position> {
        match *self {
            ErrorKind::Utf8 { ref pos, .. } => pos.as_ref(),
            ErrorKind::UnequalLengths { ref pos, .. } => pos.as_ref(),
            ErrorKind::Deserialize { ref pos, .. } => pos.as_ref(),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::new(ErrorKind::Io(err))
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> io::Error {
        io::Error::new(io::ErrorKind::Other, err)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match *self.0 {
            ErrorKind::Io(ref err) => Some(err),
            ErrorKind::Utf8 { ref err, .. } => Some(err),
            ErrorKind::UnequalLengths { .. } => None,
            ErrorKind::Seek => None,
            ErrorKind::Serialize(_) => None,
            ErrorKind::Deserialize { ref err, .. } => Some(err),
            _ => unreachable!(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self.0 {
            ErrorKind::Io(ref err) => err.fmt(f),
            ErrorKind::Utf8 { pos: None, ref err } => {
                write!(f, "CSV parse error: field {}: {}", err.field(), err)
            }
            ErrorKind::Utf8 { pos: Some(ref pos), ref err } => write!(
                f,
                "CSV parse error: record {} \
                 (line {}, field: {}, byte: {}): {}",
                pos.record(),
                pos.line(),
                err.field(),
                pos.byte(),
                err
            ),
            ErrorKind::UnequalLengths { pos: None, expected_len, len } => {
                write!(
                    f,
                    "CSV error: \
                     found record with {} fields, but the previous record \
                     has {} fields",
                    len, expected_len
                )
            }
            ErrorKind::UnequalLengths {
                pos: Some(ref pos),
                expected_len,
                len,
            } => write!(
                f,
                "CSV error: record {} (line: {}, byte: {}): \
                 found record with {} fields, but the previous record \
                 has {} fields",
                pos.record(),
                pos.line(),
                pos.byte(),
                len,
                expected_len
            ),
            ErrorKind::Seek => write!(
                f,
                "CSV error: cannot access headers of CSV data \
                 when the parser was seeked before the first record \
                 could be read"
            ),
            ErrorKind::Serialize(ref err) => {
                write!(f, "CSV write error: {}", err)
            }
            ErrorKind::Deserialize { pos: None, ref err } => {
                write!(f, "CSV deserialize error: {}", err)
            }
            ErrorKind::Deserialize { pos: Some(ref pos), ref err } => write!(
                f,
                "CSV deserialize error: record {} \
                 (line: {}, byte: {}): {}",
                pos.record(),
                pos.line(),
                pos.byte(),
                err
            ),
            _ => unreachable!(),
        }
    }
}

/// A UTF-8 validation error during record conversion.
///
/// This occurs when attempting to convert a `ByteRecord` into a
/// `StringRecord`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FromUtf8Error {
    record: ByteRecord,
    err: Utf8Error,
}

impl FromUtf8Error {
    /// Create a new FromUtf8Error.
    pub(crate) fn new(rec: ByteRecord, err: Utf8Error) -> FromUtf8Error {
        FromUtf8Error { record: rec, err: err }
    }

    /// Access the underlying `ByteRecord` that failed UTF-8 validation.
    pub fn into_byte_record(self) -> ByteRecord {
        self.record
    }

    /// Access the underlying UTF-8 validation error.
    pub fn utf8_error(&self) -> &Utf8Error {
        &self.err
    }
}

impl fmt::Display for FromUtf8Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.err.fmt(f)
    }
}

impl StdError for FromUtf8Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.err)
    }
}

/// A UTF-8 validation error.
///
/// This occurs when attempting to convert a `ByteRecord` into a
/// `StringRecord`.
///
/// The error includes the index of the field that failed validation, and the
/// last byte at which valid UTF-8 was verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Utf8Error {
    /// The field index of a byte record in which UTF-8 validation failed.
    field: usize,
    /// The index into the given field up to which valid UTF-8 was verified.
    valid_up_to: usize,
}

/// Create a new UTF-8 error.
pub fn new_utf8_error(field: usize, valid_up_to: usize) -> Utf8Error {
    Utf8Error { field: field, valid_up_to: valid_up_to }
}

impl Utf8Error {
    /// The field index of a byte record in which UTF-8 validation failed.
    pub fn field(&self) -> usize {
        self.field
    }
    /// The index into the given field up to which valid UTF-8 was verified.
    pub fn valid_up_to(&self) -> usize {
        self.valid_up_to
    }
}

impl StdError for Utf8Error {}

impl fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "invalid utf-8: invalid UTF-8 in field {} near byte index {}",
            self.field, self.valid_up_to
        )
    }
}

/// `IntoInnerError` occurs when consuming a `Writer` fails.
///
/// Consuming the `Writer` causes a flush to happen. If the flush fails, then
/// this error is returned, which contains both the original `Writer` and
/// the error that occurred.
///
/// The type parameter `W` is the unconsumed writer.
pub struct IntoInnerError<W> {
    wtr: W,
    err: io::Error,
}

impl<W> IntoInnerError<W> {
    /// Creates a new `IntoInnerError`.
    ///
    /// (This is a visibility hack. It's public in this module, but not in the
    /// crate.)
    pub(crate) fn new(wtr: W, err: io::Error) -> IntoInnerError<W> {
        IntoInnerError { wtr: wtr, err: err }
    }

    /// Returns the error which caused the call to `into_inner` to fail.
    ///
    /// This error was returned when attempting to flush the internal buffer.
    pub fn error(&self) -> &io::Error {
        &self.err
    }

    /// Returns the underlying writer which generated the error.
    ///
    /// The returned value can be used for error recovery, such as
    /// re-inspecting the buffer.
    pub fn into_inner(self) -> W {
        self.wtr
    }
}

impl<W: std::any::Any> StdError for IntoInnerError<W> {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.err.source()
    }
}

impl<W> fmt::Display for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.err.fmt(f)
    }
}

impl<W> fmt::Debug for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.err.fmt(f)
    }
}
