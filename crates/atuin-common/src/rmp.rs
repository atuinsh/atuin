//! MessagePack encode/decode helpers built on [`rmp`].
//!
//! `rmp`'s own error types are awkward: some don't implement [`Display`] for
//! every `E`, none say which variant they are, and the decode cursor offers no
//! owned-string read, no nil-aware optional read, and no structural checks
//! (array length, end-of-input). This module fills those gaps with two
//! extension traits — [`RmpDecodeExt`] for reading a [`Bytes`] cursor and
//! [`RmpEncodeExt`] for writing — plus [`DecodeError`] / [`EncodeError`], which
//! carry legible messages and convert cleanly into [`eyre::Report`].
//!
//! Because every decode read returns [`DecodeError`], a decoder can use `?`
//! throughout and convert once at the boundary, rather than hand-rolling a
//! `map_err` at every read.
//!
//! [`Display`]: std::fmt::Display

use rmp::Marker;
use rmp::decode::bytes::{Bytes, BytesReadError};
use rmp::decode::{self, DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError};
use rmp::encode::{self, RmpWrite, RmpWriteErr, ValueWriteError};

/// An error encountered while encoding a MessagePack value.
///
/// Wraps [`ValueWriteError`] with a message that names the failing write and its
/// inner I/O error — neither of which `rmp`'s own [`Display`] reports. Implements
/// [`std::error::Error`], so it converts into [`eyre::Report`] with `?`.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

/// An error encountered while decoding a MessagePack value.
///
/// Wraps the three error types `rmp`'s decode functions return, and adds two
/// structural variants for checks `rmp` does not perform itself:
/// [`UnexpectedArrayLen`](Self::UnexpectedArrayLen) and
/// [`TrailingBytes`](Self::TrailingBytes).
///
/// Converts into [`eyre::Report`] via a manual `From` rather than a
/// [`std::error::Error`] impl, because a [`DecodeError`] is not, in general,
/// `'static`.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display)]
pub enum DecodeError<'a, E: RmpReadErr = BytesReadError> {
    /// The next value was not a valid UTF-8 string.
    #[display("could not decode MessagePack string: {_0:?}")]
    DecodeString(DecodeStringError<'a, E>),
    /// The next value was not the expected number.
    #[display("could not decode MessagePack number: {_0:?}")]
    NumValueRead(NumValueReadError<E>),
    /// The next value could not be decoded.
    #[display("could not decode MessagePack value: {_0:?}")]
    ValueRead(ValueReadError<E>),
    /// An array marker did not have the expected length.
    #[display("expected a MessagePack array of length {expected}, found {actual}")]
    UnexpectedArrayLen { expected: u32, actual: u32 },
    /// Input remained after a value was fully decoded.
    #[display("{remaining} trailing byte(s) after decoding MessagePack value")]
    TrailingBytes { remaining: usize },
}

impl<'a, E: RmpReadErr> From<DecodeStringError<'a, E>> for DecodeError<'a, E> {
    fn from(e: DecodeStringError<'a, E>) -> Self {
        Self::DecodeString(e)
    }
}

impl<E: RmpReadErr> From<NumValueReadError<E>> for DecodeError<'_, E> {
    fn from(e: NumValueReadError<E>) -> Self {
        Self::NumValueRead(e)
    }
}

impl<E: RmpReadErr> From<ValueReadError<E>> for DecodeError<'_, E> {
    fn from(e: ValueReadError<E>) -> Self {
        Self::ValueRead(e)
    }
}

impl<E: RmpReadErr> DecodeError<'_, E> {
    /// If this is a type mismatch, the [`Marker`] found instead of the expected
    /// type. [`RmpDecodeExt::read_optional`] uses this to recognise nil.
    pub fn type_mismatch(&self) -> Option<Marker> {
        match self {
            Self::DecodeString(DecodeStringError::TypeMismatch(m))
            | Self::NumValueRead(NumValueReadError::TypeMismatch(m))
            | Self::ValueRead(ValueReadError::TypeMismatch(m)) => Some(*m),
            _ => None,
        }
    }
}

impl<E: RmpReadErr> From<DecodeError<'_, E>> for eyre::Report {
    fn from(e: DecodeError<'_, E>) -> Self {
        eyre::eyre!("{e}")
    }
}

/// Reading helpers for a MessagePack [`Bytes`] cursor.
///
/// Every method reports failures as [`DecodeError`], so decoders use `?`
/// throughout and convert once at the boundary.
pub trait RmpDecodeExt<'a> {
    /// Run an `rmp` decode function, converting its error into [`DecodeError`].
    ///
    /// Lets raw `rmp::decode` functions compose with `?`:
    /// `bytes.read_with(rmp::decode::read_u64)?`.
    fn read_with<T, E, F>(&mut self, read: F) -> Result<T, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>;

    /// Read an owned [`String`].
    fn read_string(&mut self) -> Result<String, DecodeError<'a>>;

    /// Read a value that may be encoded as nil, returning [`None`] for nil.
    ///
    /// `read` decodes a `T`; if it fails specifically because it found
    /// [`Marker::Null`], this yields [`None`] with the cursor left just past the
    /// nil. Any other error is forwarded unchanged.
    fn read_optional<T, E, F>(&mut self, read: F) -> Result<Option<T>, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>;

    /// Read an array-length marker.
    fn read_array_len(&mut self) -> Result<u32, DecodeError<'a>>;

    /// Read an array-length marker and require it to equal `expected`.
    ///
    /// Returns [`DecodeError::UnexpectedArrayLen`] otherwise. For a
    /// forward-compatible field count, use [`read_array_len`](Self::read_array_len)
    /// and range-check the value yourself.
    fn expect_array_len(&mut self, expected: u32) -> Result<u32, DecodeError<'a>>;

    /// Succeed only if the cursor is at end-of-input, else
    /// [`DecodeError::TrailingBytes`] — the standard malformed-record guard after
    /// a fixed set of fields.
    fn expect_eof(&self) -> Result<(), DecodeError<'a>>;
}

impl<'a> RmpDecodeExt<'a> for Bytes<'a> {
    fn read_with<T, E, F>(&mut self, read: F) -> Result<T, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>,
    {
        read(self).map_err(Into::into)
    }

    fn read_string(&mut self) -> Result<String, DecodeError<'a>> {
        let slice = self.remaining_slice();
        let (string, rest) = match decode::read_str_from_slice(slice) {
            Ok(pair) => pair,
            Err(e) => {
                if let DecodeStringError::TypeMismatch(_) = e {
                    // rmp's decode functions consume the marker byte on a type
                    // mismatch; do the same so `read_optional` can detect nil.
                    self.read_u8()
                        .expect("TypeMismatch implies the stream contains a marker byte");
                }
                return Err(e.into());
            }
        };
        *self = Bytes::new(rest);
        Ok(string.into())
    }

    fn read_optional<T, E, F>(&mut self, read: F) -> Result<Option<T>, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>,
    {
        match read(self) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                let e = e.into();
                if let Some(Marker::Null) = e.type_mismatch() {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn read_array_len(&mut self) -> Result<u32, DecodeError<'a>> {
        decode::read_array_len(self).map_err(Into::into)
    }

    fn expect_array_len(&mut self, expected: u32) -> Result<u32, DecodeError<'a>> {
        let actual = self.read_array_len()?;
        if actual == expected {
            Ok(actual)
        } else {
            Err(DecodeError::UnexpectedArrayLen { expected, actual })
        }
    }

    fn expect_eof(&self) -> Result<(), DecodeError<'a>> {
        let remaining = self.remaining_slice().len();
        if remaining == 0 {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes { remaining })
        }
    }
}

/// Writing helpers for a MessagePack output buffer.
pub trait RmpEncodeExt: RmpWrite {
    /// Write an optional value, encoding [`None`] as [`Marker::Null`].
    ///
    /// The mirror of [`RmpDecodeExt::read_optional`]: `write` encodes the inner
    /// value when `value` is [`Some`].
    fn write_optional<T, F>(
        &mut self,
        value: Option<T>,
        write: F,
    ) -> Result<(), EncodeError<Self::Error>>
    where
        F: FnOnce(&mut Self, T) -> Result<(), ValueWriteError<Self::Error>>;
}

impl<W: RmpWrite> RmpEncodeExt for W {
    fn write_optional<T, F>(
        &mut self,
        value: Option<T>,
        write: F,
    ) -> Result<(), EncodeError<Self::Error>>
    where
        F: FnOnce(&mut Self, T) -> Result<(), ValueWriteError<Self::Error>>,
    {
        match value {
            Some(v) => write(self, v).map_err(EncodeError::from),
            None => encode::write_nil(self)
                .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rmp::decode::bytes::Bytes;
    use rstest::rstest;

    // Build a real DecodeError by asking rmp to read the wrong type.
    fn type_mismatch_error<'a>() -> DecodeError<'a> {
        // 0xc0 is the nil marker; reading it as a u64 is a type mismatch.
        let mut bytes = Bytes::new(&[0xc0]);
        rmp::decode::read_u64(&mut bytes)
            .map_err(DecodeError::from)
            .unwrap_err()
    }

    #[test]
    fn type_mismatch_exposes_marker() {
        assert_eq!(type_mismatch_error().type_mismatch(), Some(rmp::Marker::Null));
    }

    #[rstest]
    #[case::array_len(
        DecodeError::<'static>::UnexpectedArrayLen { expected: 3, actual: 5 },
        None
    )]
    #[case::trailing(DecodeError::<'static>::TrailingBytes { remaining: 2 }, None)]
    fn structural_variants_have_no_marker(
        #[case] err: DecodeError<'static>,
        #[case] expected: Option<rmp::Marker>,
    ) {
        assert_eq!(err.type_mismatch(), expected);
    }

    #[test]
    fn decode_error_converts_to_eyre_with_message() {
        let report: eyre::Report =
            DecodeError::<'static, BytesReadError>::TrailingBytes { remaining: 4 }.into();
        assert!(report.to_string().contains("trailing"));
    }

    #[test]
    fn display_messages_are_legible() {
        assert_eq!(
            DecodeError::<'static, BytesReadError>::UnexpectedArrayLen { expected: 3, actual: 5 }
                .to_string(),
            "expected a MessagePack array of length 3, found 5",
        );
    }

    // Encode helpers used only by tests, to build inputs.
    fn enc<F: FnOnce(&mut Vec<u8>)>(f: F) -> Vec<u8> {
        let mut v = Vec::new();
        f(&mut v);
        v
    }

    #[test]
    fn read_string_round_trips() {
        let buf = enc(|v| rmp::encode::write_str(v, "héllo 🦀").unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(bytes.read_string().unwrap(), "héllo 🦀");
        assert!(bytes.remaining_slice().is_empty());
    }

    #[test]
    fn read_string_on_wrong_type_errors_and_consumes_marker() {
        // A lone nil marker: read_string must fail *and* consume the marker so a
        // following read_optional can observe end-of-input correctly.
        let mut bytes = Bytes::new(&[0xc0]);
        assert!(bytes.read_string().is_err());
        assert!(bytes.remaining_slice().is_empty(), "marker byte must be consumed");
    }

    #[test]
    fn read_with_converts_rmp_errors() {
        let buf = enc(|v| rmp::encode::write_u64(v, 42).unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(bytes.read_with(rmp::decode::read_u64).unwrap(), 42);
    }

    #[test]
    fn read_optional_some_and_none() {
        let some = enc(|v| rmp::encode::write_u64(v, 7).unwrap());
        let mut b = Bytes::new(&some);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), Some(7));

        let none = enc(|v| rmp::encode::write_nil(v).unwrap());
        let mut b = Bytes::new(&none);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), None);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_optional_string_via_closure() {
        let buf = enc(|v| rmp::encode::write_str(v, "x").unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(b.read_optional(|b| b.read_string()).unwrap(), Some("x".to_string()));
    }

    #[test]
    fn read_optional_forwards_non_nil_errors() {
        // A bool where a u64 is expected is a type mismatch that is NOT nil.
        let buf = enc(|v| rmp::encode::write_bool(v, true).unwrap());
        let mut b = Bytes::new(&buf);
        assert!(b.read_optional(rmp::decode::read_u64).is_err());
    }

    fn array_of(len: u32) -> Vec<u8> {
        enc(|v| {
            rmp::encode::write_array_len(v, len).unwrap();
        })
    }

    #[test]
    fn expect_array_len_exact_ok() {
        let buf = array_of(3);
        let mut b = Bytes::new(&buf);
        assert_eq!(b.expect_array_len(3).unwrap(), 3);
    }

    #[test]
    fn expect_array_len_mismatch_reports_expected_and_actual() {
        let buf = array_of(5);
        let mut b = Bytes::new(&buf);
        match b.expect_array_len(3) {
            Err(DecodeError::UnexpectedArrayLen { expected, actual }) => {
                assert_eq!((expected, actual), (3, 5));
            }
            other => panic!("expected UnexpectedArrayLen, got {other:?}"),
        }
    }

    #[test]
    fn read_array_len_returns_count_for_manual_range_checks() {
        let buf = array_of(9);
        let mut b = Bytes::new(&buf);
        assert_eq!(b.read_array_len().unwrap(), 9);
    }

    #[test]
    fn expect_eof_ok_when_consumed() {
        let buf = enc(|v| rmp::encode::write_u8(v, 1).unwrap());
        let mut b = Bytes::new(&buf);
        b.read_with(rmp::decode::read_u8).unwrap();
        assert!(b.expect_eof().is_ok());
    }

    #[test]
    fn expect_eof_reports_remaining() {
        let b = Bytes::new(&[0x01, 0x02, 0x03]);
        match b.expect_eof() {
            Err(DecodeError::TrailingBytes { remaining }) => assert_eq!(remaining, 3),
            other => panic!("expected TrailingBytes, got {other:?}"),
        }
    }
}
