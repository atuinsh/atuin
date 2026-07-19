//! MessagePack encode/decode helpers built on [`rmp`].
//!
//! `rmp`'s own error types are awkward: some don't implement [`Display`] for
//! every `E`, none say which variant they are, and the decode cursor offers no
//! owned-string read, no nil-aware optional read, and no structural checks
//! (array length, end-of-input). This module fills those gaps.
//!
//! Values are read through the generic [`decode()`] function, backed by the
//! [`Decode`] trait: `decode::<String>(&mut bytes)?`,
//! `decode::<Option<u64>>(&mut bytes)?`. Structural markers that aren't values
//! have their own functions — [`decode_array_len`], [`decode_bin_len`],
//! [`expect_array_len`] and [`expect_eof`] — and [`write_optional`] handles the
//! encode side. All of these return [`DecodeError`] / [`EncodeError`], which
//! carry legible messages and convert cleanly into [`eyre::Report`].
//!
//! Because every decode read returns [`DecodeError`], a decoder can use `?`
//! throughout and convert once at the boundary, rather than hand-rolling a
//! `map_err` at every read.
//!
//! [`Display`]: std::fmt::Display

use rmp::Marker;
use rmp::decode::bytes::BytesReadError;
use rmp::decode::{
    self, DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError,
};
use rmp::encode::{self, RmpWrite, RmpWriteErr, ValueWriteError};

/// Re-exported so decoders need only depend on this module, not `rmp::decode`.
pub use rmp::decode::bytes::Bytes;

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
    /// type. The [`Option`] implementation of [`Decode`] uses this to recognise
    /// nil.
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

/// A value decodable from a MessagePack [`Bytes`] cursor.
///
/// Implemented for the scalars atuin's record formats use and for [`Option`]
/// (nil ⇒ [`None`]). Call it through the generic [`decode()`] function; structural
/// markers that aren't values have their own functions ([`decode_array_len`],
/// [`decode_bin_len`]).
pub trait Decode: Sized {
    fn decode<'a>(bytes: &mut Bytes<'a>) -> Result<Self, DecodeError<'a>>;
}

/// Decode a [`Decode`] value: `decode::<String>(&mut bytes)?`,
/// `decode::<Option<u64>>(&mut bytes)?`.
pub fn decode<'a, T: Decode>(bytes: &mut Bytes<'a>) -> Result<T, DecodeError<'a>> {
    T::decode(bytes)
}

impl Decode for String {
    fn decode<'a>(bytes: &mut Bytes<'a>) -> Result<Self, DecodeError<'a>> {
        let slice = bytes.remaining_slice();
        let (string, rest) = match decode::read_str_from_slice(slice) {
            Ok(pair) => pair,
            Err(e) => {
                if let DecodeStringError::TypeMismatch(_) = e {
                    // rmp consumes the marker byte on a type mismatch; match that
                    // so `Option::decode` can detect nil.
                    bytes
                        .read_u8()
                        .expect("TypeMismatch implies the stream contains a marker byte");
                }
                return Err(e.into());
            }
        };
        *bytes = Bytes::new(rest);
        Ok(string.into())
    }
}

impl Decode for bool {
    fn decode<'a>(bytes: &mut Bytes<'a>) -> Result<Self, DecodeError<'a>> {
        decode::read_bool(bytes).map_err(Into::into)
    }
}

macro_rules! impl_decode_int {
    ($($t:ty),+ $(,)?) => {$(
        impl Decode for $t {
            fn decode<'a>(bytes: &mut Bytes<'a>) -> Result<Self, DecodeError<'a>> {
                decode::read_int(bytes).map_err(Into::into)
            }
        }
    )+};
}
impl_decode_int!(u8, u16, u64, i64);

impl<T: Decode> Decode for Option<T> {
    /// Decodes a `T`, mapping a nil marker to [`None`].
    fn decode<'a>(bytes: &mut Bytes<'a>) -> Result<Self, DecodeError<'a>> {
        match T::decode(bytes) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if let Some(Marker::Null) = e.type_mismatch() {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Decode an array-length marker (the header count, not a value).
pub fn decode_array_len<'a>(bytes: &mut Bytes<'a>) -> Result<u32, DecodeError<'a>> {
    decode::read_array_len(bytes).map_err(Into::into)
}

/// Decode a binary-length marker (the header, before the raw payload bytes).
pub fn decode_bin_len<'a>(bytes: &mut Bytes<'a>) -> Result<u32, DecodeError<'a>> {
    decode::read_bin_len(bytes).map_err(Into::into)
}

/// Decode an array-length marker and require it to equal `expected`, else
/// [`DecodeError::UnexpectedArrayLen`]. For a forward-compatible field count,
/// use [`decode_array_len`] and range-check yourself.
pub fn expect_array_len<'a>(bytes: &mut Bytes<'a>, expected: u32) -> Result<u32, DecodeError<'a>> {
    let actual = decode_array_len(bytes)?;
    if actual == expected {
        Ok(actual)
    } else {
        Err(DecodeError::UnexpectedArrayLen { expected, actual })
    }
}

/// Succeed only if the cursor is at end-of-input, else [`DecodeError::TrailingBytes`].
pub fn expect_eof<'a>(bytes: &Bytes<'a>) -> Result<(), DecodeError<'a>> {
    let remaining = bytes.remaining_slice().len();
    if remaining == 0 {
        Ok(())
    } else {
        Err(DecodeError::TrailingBytes { remaining })
    }
}

/// Write an optional value, encoding [`None`] as [`Marker::Null`].
pub fn write_optional<W: RmpWrite, T>(
    out: &mut W,
    value: Option<T>,
    write: impl FnOnce(&mut W, T) -> Result<(), ValueWriteError<W::Error>>,
) -> Result<(), EncodeError<W::Error>> {
    match value {
        Some(v) => write(out, v).map_err(EncodeError::from),
        None => encode::write_nil(out)
            .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e))),
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
        assert_eq!(
            type_mismatch_error().type_mismatch(),
            Some(rmp::Marker::Null)
        );
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
            DecodeError::<'static, BytesReadError>::UnexpectedArrayLen {
                expected: 3,
                actual: 5
            }
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
    fn decode_string_round_trips() {
        let buf = enc(|v| rmp::encode::write_str(v, "héllo 🦀").unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(decode::<String>(&mut bytes).unwrap(), "héllo 🦀");
        assert!(bytes.remaining_slice().is_empty());
    }

    #[test]
    fn decode_string_on_wrong_type_errors_and_consumes_marker() {
        // A lone nil marker: decode::<String> must fail *and* consume the marker
        // so a following decode::<Option<_>> can observe end-of-input correctly.
        let mut bytes = Bytes::new(&[0xc0]);
        assert!(decode::<String>(&mut bytes).is_err());
        assert!(
            bytes.remaining_slice().is_empty(),
            "marker byte must be consumed"
        );
    }

    #[test]
    fn decode_converts_rmp_errors() {
        let buf = enc(|v| rmp::encode::write_u64(v, 42).unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(decode::<u64>(&mut bytes).unwrap(), 42);
    }

    #[rstest]
    #[case::bool_true(enc(|v| rmp::encode::write_bool(v, true).unwrap()), true)]
    #[case::bool_false(enc(|v| rmp::encode::write_bool(v, false).unwrap()), false)]
    fn decode_bool_round_trips(#[case] buf: Vec<u8>, #[case] expected: bool) {
        let mut b = Bytes::new(&buf);
        assert_eq!(decode::<bool>(&mut b).unwrap(), expected);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn decode_u8_round_trips() {
        let buf = enc(|v| rmp::encode::write_u8(v, 200).unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(decode::<u8>(&mut b).unwrap(), 200);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn decode_u16_round_trips() {
        let buf = enc(|v| rmp::encode::write_u16(v, 40000).unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(decode::<u16>(&mut b).unwrap(), 40000);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn decode_i64_round_trips() {
        let buf = enc(|v| {
            rmp::encode::write_sint(v, -123456789).unwrap();
        });
        let mut b = Bytes::new(&buf);
        assert_eq!(decode::<i64>(&mut b).unwrap(), -123456789);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn decode_optional_some_and_none() {
        let some = enc(|v| rmp::encode::write_u64(v, 7).unwrap());
        let mut b = Bytes::new(&some);
        assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), Some(7));

        let none = enc(|v| rmp::encode::write_nil(v).unwrap());
        let mut b = Bytes::new(&none);
        assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), None);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn decode_optional_string() {
        let buf = enc(|v| rmp::encode::write_str(v, "x").unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(
            decode::<Option<String>>(&mut b).unwrap(),
            Some("x".to_string())
        );
    }

    #[test]
    fn decode_optional_forwards_non_nil_errors() {
        // A bool where a u64 is expected is a type mismatch that is NOT nil.
        let buf = enc(|v| rmp::encode::write_bool(v, true).unwrap());
        let mut b = Bytes::new(&buf);
        assert!(decode::<Option<u64>>(&mut b).is_err());
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
        assert_eq!(expect_array_len(&mut b, 3).unwrap(), 3);
    }

    #[test]
    fn expect_array_len_mismatch_reports_expected_and_actual() {
        let buf = array_of(5);
        let mut b = Bytes::new(&buf);
        match expect_array_len(&mut b, 3) {
            Err(DecodeError::UnexpectedArrayLen { expected, actual }) => {
                assert_eq!((expected, actual), (3, 5));
            }
            other => panic!("expected UnexpectedArrayLen, got {other:?}"),
        }
    }

    #[test]
    fn decode_array_len_returns_count_for_manual_range_checks() {
        let buf = array_of(9);
        let mut b = Bytes::new(&buf);
        assert_eq!(decode_array_len(&mut b).unwrap(), 9);
    }

    #[test]
    fn expect_eof_ok_when_consumed() {
        let buf = enc(|v| rmp::encode::write_u8(v, 1).unwrap());
        let mut b = Bytes::new(&buf);
        decode::<u8>(&mut b).unwrap();
        assert!(expect_eof(&b).is_ok());
    }

    #[test]
    fn expect_eof_reports_remaining() {
        let b = Bytes::new(&[0x01, 0x02, 0x03]);
        match expect_eof(&b) {
            Err(DecodeError::TrailingBytes { remaining }) => assert_eq!(remaining, 3),
            other => panic!("expected TrailingBytes, got {other:?}"),
        }
    }

    #[test]
    fn write_optional_some_then_read_back() {
        let mut out = Vec::new();
        write_optional(&mut out, Some(99u64), rmp::encode::write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), Some(99));
    }

    #[test]
    fn write_optional_none_writes_nil() {
        let mut out = Vec::new();
        write_optional::<_, u64>(&mut out, None, rmp::encode::write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), None);
    }

    #[test]
    fn write_optional_str() {
        let mut out = Vec::new();
        write_optional(&mut out, Some("hi"), rmp::encode::write_str).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(
            decode::<Option<String>>(&mut b).unwrap(),
            Some("hi".to_string())
        );
    }

    #[test]
    fn decode_optional_nil_string_advances_to_next_field() {
        // A nil optional-string field followed by a u64. The nil read must consume
        // exactly the marker so the following field decodes correctly.
        let mut out = Vec::new();
        write_optional::<_, &str>(&mut out, None, rmp::encode::write_str).unwrap();
        rmp::encode::write_u64(&mut out, 1234).unwrap();

        let mut b = Bytes::new(&out);
        assert_eq!(decode::<Option<String>>(&mut b).unwrap(), None);
        assert_eq!(decode::<u64>(&mut b).unwrap(), 1234);
        assert!(expect_eof(&b).is_ok());
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        // A string survives an encode -> decode::<String> round trip exactly.
        #[test]
        fn decode_string_proptest_round_trips(s in r"(?s).*") {
            let buf = enc(|v| rmp::encode::write_str(v, &s).unwrap());
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(decode::<String>(&mut b).unwrap(), s);
            prop_assert!(b.remaining_slice().is_empty());
        }

        // Option<u64> survives write_optional -> decode::<Option<u64>>.
        #[test]
        fn decode_optional_u64_round_trips(v in proptest::option::of(any::<u64>())) {
            let mut out = Vec::new();
            write_optional(&mut out, v, rmp::encode::write_u64).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), v);
        }

        // Option<String> survives the optional round trip.
        #[test]
        fn decode_optional_string_round_trips(v in proptest::option::of(r"(?s).*")) {
            let mut out = Vec::new();
            write_optional(&mut out, v.as_deref(), rmp::encode::write_str).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(decode::<Option<String>>(&mut b).unwrap(), v);
            prop_assert!(b.remaining_slice().is_empty());
        }

        // A full array record (len + fields + optional tail) round trips, and the
        // cursor is exactly exhausted afterwards.
        #[test]
        fn record_round_trips(
            id in r"[a-z]{0,16}",
            ts in any::<u64>(),
            deleted in proptest::option::of(any::<u64>()),
        ) {
            let mut out = Vec::new();
            rmp::encode::write_array_len(&mut out, 3).unwrap();
            rmp::encode::write_str(&mut out, &id).unwrap();
            rmp::encode::write_u64(&mut out, ts).unwrap();
            write_optional(&mut out, deleted, rmp::encode::write_u64).unwrap();

            let mut b = Bytes::new(&out);
            prop_assert_eq!(expect_array_len(&mut b, 3).unwrap(), 3);
            prop_assert_eq!(decode::<String>(&mut b).unwrap(), id);
            prop_assert_eq!(decode::<u64>(&mut b).unwrap(), ts);
            prop_assert_eq!(decode::<Option<u64>>(&mut b).unwrap(), deleted);
            prop_assert!(expect_eof(&b).is_ok());
        }

        // Reads never panic on arbitrary bytes — they return Err instead.
        #[test]
        fn reads_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut b = Bytes::new(&raw);
            let _ = decode::<String>(&mut b);
            let mut b = Bytes::new(&raw);
            let _ = decode_array_len(&mut b);
            let mut b = Bytes::new(&raw);
            let _ = decode::<Option<u64>>(&mut b);
        }
    }
}
