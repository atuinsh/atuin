//! MessagePack decode helpers built on [`rmp::decode`].
//!
//! Mirrors `rmp::decode`'s `read_*` shape, but every read returns our own
//! [`DecodeError`] (which converts cleanly into [`eyre::Report`]) so a decoder
//! can use `?` throughout and convert once at the boundary. Adds an owned
//! [`read_string`], a nil-aware [`read_optional`], and the structural checks
//! [`read_array_len`]/[`expect_array_len`] and [`expect_eof`] that `rmp` does
//! not perform itself.

use rmp::Marker;
use rmp::decode as mp;
use rmp::decode::bytes::BytesReadError;
use rmp::decode::{DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError};

/// Re-exported so decoders need only depend on this module, not `rmp::decode`.
pub use rmp::decode::bytes::Bytes;

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
    /// type. [`read_optional`] uses this to recognise nil.
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

macro_rules! read_int {
    ($($(#[$meta:meta])* $name:ident -> $t:ty),+ $(,)?) => {$(
        $(#[$meta])*
        pub fn $name<'a>(bytes: &mut Bytes<'a>) -> Result<$t, DecodeError<'a>> {
            mp::read_int(bytes).map_err(Into::into)
        }
    )+};
}

read_int! {
    /// Read a `u8` value. Accepts any in-range MessagePack integer encoding.
    read_u8 -> u8,
    /// Read a `u16` value. Accepts any in-range MessagePack integer encoding.
    read_u16 -> u16,
    /// Read a `u64` value. Accepts any in-range MessagePack integer encoding.
    read_u64 -> u64,
    /// Read an `i64` value. Accepts any in-range MessagePack integer encoding.
    read_i64 -> i64,
}

/// Read a `bool`.
pub fn read_bool<'a>(bytes: &mut Bytes<'a>) -> Result<bool, DecodeError<'a>> {
    mp::read_bool(bytes).map_err(Into::into)
}

/// Read a binary-blob length header (before the raw payload).
pub fn read_bin_len<'a>(bytes: &mut Bytes<'a>) -> Result<u32, DecodeError<'a>> {
    mp::read_bin_len(bytes).map_err(Into::into)
}

/// Read an array-length header.
pub fn read_array_len<'a>(bytes: &mut Bytes<'a>) -> Result<u32, DecodeError<'a>> {
    mp::read_array_len(bytes).map_err(Into::into)
}

/// Read an owned [`String`].
pub fn read_string<'a>(bytes: &mut Bytes<'a>) -> Result<String, DecodeError<'a>> {
    let slice = bytes.remaining_slice();
    let (string, rest) = match mp::read_str_from_slice(slice) {
        Ok(pair) => pair,
        Err(e) => {
            if let DecodeStringError::TypeMismatch(_) = e {
                // rmp consumes the marker byte on a type mismatch; match that so
                // `read_optional` can detect nil.
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

/// Read a value that may be nil, returning [`None`] for nil. `read` decodes the
/// inner value (e.g. [`read_u64`] or [`read_string`]).
pub fn read_optional<'a, T, E>(
    bytes: &mut Bytes<'a>,
    read: impl FnOnce(&mut Bytes<'a>) -> Result<T, E>,
) -> Result<Option<T>, DecodeError<'a>>
where
    E: Into<DecodeError<'a>>,
{
    match read(bytes) {
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

/// Read an array-length header and require it to equal `expected`, else
/// [`DecodeError::UnexpectedArrayLen`]. For a forward-compatible field count,
/// use [`read_array_len`] and range-check yourself.
pub fn expect_array_len<'a>(bytes: &mut Bytes<'a>, expected: u32) -> Result<u32, DecodeError<'a>> {
    let actual = read_array_len(bytes)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rmp::encode;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // Build a real DecodeError by asking rmp to read the wrong type.
    fn type_mismatch_error<'a>() -> DecodeError<'a> {
        // 0xc0 is the nil marker; reading it as a u64 is a type mismatch.
        let mut bytes = Bytes::new(&[0xc0]);
        read_u64(&mut bytes).unwrap_err()
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
    fn read_string_round_trips() {
        let buf = enc(|v| encode::write_str(v, "héllo 🦀").unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(read_string(&mut bytes).unwrap(), "héllo 🦀");
        assert!(bytes.remaining_slice().is_empty());
    }

    #[test]
    fn read_string_on_wrong_type_errors_and_consumes_marker() {
        // A lone nil marker: read_string must fail *and* consume the marker so a
        // following read_optional can observe end-of-input correctly.
        let mut bytes = Bytes::new(&[0xc0]);
        assert!(read_string(&mut bytes).is_err());
        assert!(
            bytes.remaining_slice().is_empty(),
            "marker byte must be consumed"
        );
    }

    #[test]
    fn read_converts_rmp_errors() {
        let buf = enc(|v| encode::write_u64(v, 42).unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(read_u64(&mut bytes).unwrap(), 42);
    }

    #[rstest]
    #[case::bool_true(enc(|v| encode::write_bool(v, true).unwrap()), true)]
    #[case::bool_false(enc(|v| encode::write_bool(v, false).unwrap()), false)]
    fn read_bool_round_trips(#[case] buf: Vec<u8>, #[case] expected: bool) {
        let mut b = Bytes::new(&buf);
        assert_eq!(read_bool(&mut b).unwrap(), expected);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_u8_round_trips() {
        let buf = enc(|v| encode::write_u8(v, 200).unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(read_u8(&mut b).unwrap(), 200);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_u16_round_trips() {
        let buf = enc(|v| encode::write_u16(v, 40000).unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(read_u16(&mut b).unwrap(), 40000);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_u64_round_trips() {
        let buf = enc(|v| encode::write_u64(v, 42).unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(read_u64(&mut b).unwrap(), 42);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_i64_round_trips() {
        let buf = enc(|v| {
            encode::write_sint(v, -123456789).unwrap();
        });
        let mut b = Bytes::new(&buf);
        assert_eq!(read_i64(&mut b).unwrap(), -123456789);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_optional_some_and_none() {
        let some = enc(|v| encode::write_u64(v, 7).unwrap());
        let mut b = Bytes::new(&some);
        assert_eq!(read_optional(&mut b, read_u64).unwrap(), Some(7));

        let none = enc(|v| rmp::encode::write_nil(v).unwrap());
        let mut b = Bytes::new(&none);
        assert_eq!(read_optional(&mut b, read_u64).unwrap(), None);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_optional_string() {
        let buf = enc(|v| encode::write_str(v, "x").unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(
            read_optional(&mut b, read_string).unwrap(),
            Some("x".to_string())
        );
    }

    #[test]
    fn read_optional_forwards_non_nil_errors() {
        // A bool where a u64 is expected is a type mismatch that is NOT nil.
        let buf = enc(|v| encode::write_bool(v, true).unwrap());
        let mut b = Bytes::new(&buf);
        assert!(read_optional(&mut b, read_u64).is_err());
    }

    fn array_of(len: u32) -> Vec<u8> {
        enc(|v| {
            encode::write_array_len(v, len).unwrap();
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
    fn read_array_len_returns_count_for_manual_range_checks() {
        let buf = array_of(9);
        let mut b = Bytes::new(&buf);
        assert_eq!(read_array_len(&mut b).unwrap(), 9);
    }

    #[test]
    fn expect_eof_ok_when_consumed() {
        let buf = enc(|v| encode::write_u8(v, 1).unwrap());
        let mut b = Bytes::new(&buf);
        read_u8(&mut b).unwrap();
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
    fn read_optional_nil_string_advances_to_next_field() {
        // A nil optional-string field followed by a u64. The nil read must consume
        // exactly the marker so the following field decodes correctly.
        let mut out = Vec::new();
        encode::write_optional::<_, &str>(&mut out, None, encode::write_str).unwrap();
        encode::write_u64(&mut out, 1234).unwrap();

        let mut b = Bytes::new(&out);
        assert_eq!(read_optional(&mut b, read_string).unwrap(), None);
        assert_eq!(read_u64(&mut b).unwrap(), 1234);
        assert!(expect_eof(&b).is_ok());
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        // A string survives an encode -> read_string round trip exactly.
        #[test]
        fn read_string_proptest_round_trips(s in r"(?s).*") {
            let buf = enc(|v| encode::write_str(v, &s).unwrap());
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(read_string(&mut b).unwrap(), s);
            prop_assert!(b.remaining_slice().is_empty());
        }

        // Option<u64> survives write_optional -> read_optional.
        #[test]
        fn read_optional_u64_round_trips(v in proptest::option::of(any::<u64>())) {
            let mut out = Vec::new();
            encode::write_optional(&mut out, v, encode::write_u64).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(read_optional(&mut b, read_u64).unwrap(), v);
        }

        // Option<String> survives the optional round trip.
        #[test]
        fn read_optional_string_round_trips(v in proptest::option::of(r"(?s).*")) {
            let mut out = Vec::new();
            encode::write_optional(&mut out, v.as_deref(), encode::write_str).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(read_optional(&mut b, read_string).unwrap(), v);
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
            encode::write_array_len(&mut out, 3).unwrap();
            encode::write_str(&mut out, &id).unwrap();
            encode::write_u64(&mut out, ts).unwrap();
            encode::write_optional(&mut out, deleted, encode::write_u64).unwrap();

            let mut b = Bytes::new(&out);
            prop_assert_eq!(expect_array_len(&mut b, 3).unwrap(), 3);
            prop_assert_eq!(read_string(&mut b).unwrap(), id);
            prop_assert_eq!(read_u64(&mut b).unwrap(), ts);
            prop_assert_eq!(read_optional(&mut b, read_u64).unwrap(), deleted);
            prop_assert!(expect_eof(&b).is_ok());
        }

        // Reads never panic on arbitrary bytes — they return Err instead.
        #[test]
        fn reads_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut b = Bytes::new(&raw);
            let _ = read_string(&mut b);
            let mut b = Bytes::new(&raw);
            let _ = read_array_len(&mut b);
            let mut b = Bytes::new(&raw);
            let _ = read_optional(&mut b, read_u64);
        }
    }
}
