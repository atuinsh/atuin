//! MessagePack decode helpers built on [`rmp::decode`].

use rmp::decode as mp;
use rmp::decode::bytes::BytesReadError;
use rmp::decode::{DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError};

/// Re-exported so decoders need only depend on this module, not `rmp::decode`.
pub use rmp::decode::bytes::Bytes;

/// Re-exported so decoders can match on markers via this module, not `rmp`.
pub use rmp::Marker;

/// An error encountered while decoding a MessagePack value.
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

pub trait DecodeExt<'a, T> {
    fn decode(self) -> Result<T, DecodeError<'a>>;
}

impl<'a, T, E: Into<DecodeError<'a>>> DecodeExt<'a, T> for Result<T, E> {
    fn decode(self) -> Result<T, DecodeError<'a>> {
        self.map_err(Into::into)
    }
}

/// Read an owned [`String`].
pub fn read_string<'a>(
    bytes: &mut Bytes<'a>,
) -> Result<String, DecodeStringError<'a, BytesReadError>> {
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
            return Err(e);
        }
    };
    *bytes = Bytes::new(rest);
    Ok(string.into())
}

/// Read a value that may be nil, returning [`None`] for nil.
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

/// Decode a MessagePack array that is expected to be the entire remaining input.
///
/// Asserts the next value is an array of exactly `len` elements, runs `read` to decode them, then
/// asserts no trailing bytes remain.
pub fn read_total_array<'a, T, E>(
    bytes: &mut Bytes<'a>,
    len: u32,
    read: impl FnOnce(&mut Bytes<'a>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<DecodeError<'a>>,
{
    expect_array_len(bytes, len).map_err(E::from)?;
    let value = read(bytes)?;
    expect_eof(bytes).map_err(E::from)?;
    Ok(value)
}

/// Read a length-prefixed MessagePack array, decoding each element with `read_elem`.
///
/// Unlike [`read_total_array`], this does not assert end-of-input.
pub fn read_array_of<'a, T, E>(
    bytes: &mut Bytes<'a>,
    mut read_elem: impl FnMut(&mut Bytes<'a>) -> Result<T, E>,
) -> Result<Vec<T>, E>
where
    E: From<DecodeError<'a>>,
{
    let len = mp::read_array_len(bytes).decode().map_err(E::from)?;
    (0..len).map(|_| read_elem(bytes)).collect()
}

/// Read an array-length header and require it to equal `expected`, else
/// [`DecodeError::UnexpectedArrayLen`].
///
/// For a forward-compatible field count, use [`rmp::decode::read_array_len`] and range-check
/// yourself. For a
/// record that is exactly a whole top-level array, prefer [`read_total_array`].
pub fn expect_array_len<'a>(bytes: &mut Bytes<'a>, expected: u32) -> Result<u32, DecodeError<'a>> {
    let actual = mp::read_array_len(bytes)?;
    if actual == expected {
        Ok(actual)
    } else {
        Err(DecodeError::UnexpectedArrayLen { expected, actual })
    }
}

/// Succeed only if the cursor is at end-of-input, else [`DecodeError::TrailingBytes`].
///
/// For a record that is exactly a whole top-level array, prefer [`read_total_array`].
pub fn expect_eof<'a>(bytes: &Bytes<'a>) -> Result<(), DecodeError<'a>> {
    let remaining = bytes.remaining_slice().len();
    if remaining == 0 {
        Ok(())
    } else {
        Err(DecodeError::TrailingBytes { remaining })
    }
}
