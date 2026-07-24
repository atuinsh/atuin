use rmp::Marker;
use rmp::decode::bytes::{Bytes, BytesReadError};
use rmp::decode::{
    self, DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError,
};
use rmp::encode::{self, RmpWrite, RmpWriteErr, ValueWriteError};

/// An error encountered while trying to encode a message with [`rmp`].
///
/// This is currently just a wrapper around [`ValueWriteError`] with a better error message.
/// [`rmp`]'s error message does not indicate which variant the error is (`InvalidMarkerWrite` or
/// `InvalidDataWrite`) and does not print anything about the inner I/O error of type `E`.
#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

/// An error encountered while trying to decode a message with [`rmp`].
///
/// This is a wrapper the various types of errors that can be returned by [`rmp`]'s decoding
/// functions. Unlike those types, this type implements [`Display`] with an error message that
/// indicates which variant the error is ([`rmp`]'s error types are enums; some unconditionally
/// print a static string and others don't even implement [`Display`] for all `E`).
///
/// Conversion to [`eyre::Report`] is supported. This cannot be done by implementing
/// [`std::error::Error`] because this type is not, in general, `'static`, so a manual
/// implementation is provided.
///
/// [`Display`]: fmt::Display
#[derive(Debug, derive_more::Display, derive_more::From)]
#[display("could not decode MessagePack value: {_0:?}")]
pub enum DecodeError<'a, E: RmpReadErr = BytesReadError> {
    DecodeString(DecodeStringError<'a, E>),
    NumValueRead(NumValueReadError<E>),
    ValueRead(ValueReadError<E>),
}

impl<E: RmpReadErr> DecodeError<'_, E> {
    pub fn type_mismatch(&self) -> Option<Marker> {
        match self {
            Self::DecodeString(DecodeStringError::TypeMismatch(m)) => Some(*m),
            Self::NumValueRead(NumValueReadError::TypeMismatch(m)) => Some(*m),
            Self::ValueRead(ValueReadError::TypeMismatch(m)) => Some(*m),
            _ => None,
        }
    }
}

impl<E: RmpReadErr> From<DecodeError<'_, E>> for eyre::Report {
    fn from(e: DecodeError<'_, E>) -> Self {
        eyre::eyre!("{e}")
    }
}

/// Read an owned string from a [`Bytes`] object.
///
/// If you need an owned [`String`], this function is more convenient than using
/// [`read_str_from_slice`] and converting the resulting [`str`], as you don't need to
/// keep unwrapping and re-creating the [`Bytes`] object.
///
/// [`read_str_from_slice`]: decode::read_str_from_slice
pub fn read_string<'a>(bytes: &mut Bytes<'a>) -> Result<String, DecodeError<'a>> {
    let slice = bytes.remaining_slice();
    let (string, rest) = match decode::read_str_from_slice(slice) {
        Ok(pair) => pair,
        Err(e) => {
            if let DecodeStringError::TypeMismatch(_) = e {
                // The decode functions in `rmp::decode` consume the marker byte when there's a
                // type mismatch; make sure we do that too, as `read_optional` depends on it.
                bytes
                    .read_u8()
                    .expect("TypeMismatch implies stream contains a marker byte");
            }
            return Err(e.into());
        }
    };
    *bytes = Bytes::new(rest);
    Ok(string.into())
}

/// Read an optional value from the stream.
///
/// This function calls `read`, which should try to decode a value of type `T` from the stream. If
/// that function returns an error indicating [`Marker::Null`] was encountered instead, this
/// function returns [`None`]. All other errors are forwarded as-is.
pub fn read_optional<'a, R, F, T, E>(
    input: &mut R,
    read: F,
) -> Result<Option<T>, DecodeError<'a, R::Error>>
where
    R: RmpRead,
    R::Error: Send + Sync,
    F: FnOnce(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    let err = match read(input) {
        Ok(v) => return Ok(Some(v)),
        Err(e) => e.into(),
    };

    if let Some(Marker::Null) = err.type_mismatch() {
        Ok(None)
    } else {
        Err(err)
    }
}

/// Write an optional value to the stream.
///
/// If `value` is [`Some`], this function calls `write` with the value, which should encode a value
/// of type `T` to the stream. Otherwise, this function writes [`Marker::Null`].
pub fn write_optional<W, T, F>(
    output: &mut W,
    value: Option<T>,
    write: F,
) -> Result<(), ValueWriteError<W::Error>>
where
    W: RmpWrite,
    F: FnOnce(&mut W, T) -> Result<(), ValueWriteError<W::Error>>,
    W::Error: Send + Sync,
{
    match value {
        Some(v) => write(output, v),
        None => encode::write_nil(output).map_err(ValueWriteError::InvalidMarkerWrite),
    }
}
