use std::convert::Infallible;

use rmp::Marker;
pub use rmp::encode::{ByteBuf, RmpWrite, RmpWriteErr, ValueWriteError};
pub use rmp::encode::{write_array_len, write_bin_len, write_map_len, write_str_len};
pub use rmp::encode::{write_bin, write_bool, write_nil, write_str};
pub use rmp::encode::{write_i8, write_i16, write_i32, write_i64};
pub use rmp::encode::{write_sint, write_uint, write_uint8};
pub use rmp::encode::{write_u8, write_u16, write_u32, write_u64};

/// An error encountered while trying to encode a message with [`rmp`].
///
/// This is currently just a wrapper around [`ValueWriteError`] with a better error message.
/// [`rmp`]'s error message does not indicate which variant the error is (`InvalidMarkerWrite` or
/// `InvalidDataWrite`) and does not print anything about the inner I/O error of type `E`.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError<E: RmpWriteErr = Infallible> {
    #[error("could not write MessagePack value: {0:?}")]
    ValueWrite(#[from] ValueWriteError<E>),
    #[error("cannot encode array larger than {}", u32::MAX)]
    ArrayOverflow,
    #[error("{0}")]
    Custom(String),
}

/// Write an optional value to the stream.
///
/// If `value` is [`Some`], this function calls `write` with the value, which should encode a value
/// of type `T` to the stream. Otherwise, this function writes [`Marker::Null`].
///
/// Because of the way this function works, the first byte written by `write` must *never* be
/// [`Marker::Null`]. That would introduce ambiguity as to whether `value` was [`None`], or `value`
/// was [`Some`] but began with a null marker.
pub fn write_optional<W, T, F, E>(
    writer: &mut W,
    value: Option<T>,
    write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    F: FnOnce(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    match value {
        Some(v) => write(writer, v).map_err(Into::into),
        None => write_nil(writer).map_err(|e| ValueWriteError::InvalidMarkerWrite(e).into()),
    }
}

/// Write an iterator of elements as a MessagePack array.
///
/// `write` is a function that writes a single element.
pub fn write_array<W, S, T, F, E>(
    writer: &mut W,
    iter: S,
    write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    S: IntoIterator<IntoIter: ExactSizeIterator<Item = T>>,
    F: FnMut(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    try_write_array(writer, iter.into_iter().map(Ok::<_, Infallible>), write).map_err(Into::into)
}

/// Write an iterator of bytes as a MessagePack binary array.
pub fn write_binary_array<W, B>(writer: &mut W, bytes: B) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    B: IntoIterator<IntoIter: ExactSizeIterator<Item = u8>>,
{
    try_write_array_impl(
        writer,
        bytes.into_iter().map(Ok::<_, Infallible>),
        write_bin_len,
        |writer, byte| {
            writer
                .write_u8(byte)
                .map_err(ValueWriteError::InvalidDataWrite)
        },
    )
    .map_err(Into::into)
}

/// Error returned by [`try_write_array`].
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum TryEncodeError<E: RmpWriteErr, Inner> {
    #[error("encoding error: {0}")]
    Encode(EncodeError<E>),
    #[error("iterator yielded an error: {0}")]
    Inner(Inner),
}

impl<E: RmpWriteErr, Inner> From<EncodeError<E>> for TryEncodeError<E, Inner> {
    fn from(e: EncodeError<E>) -> Self {
        Self::Encode(e)
    }
}

impl<E: RmpWriteErr> From<TryEncodeError<E, Infallible>> for EncodeError<E> {
    fn from(e: TryEncodeError<E, Infallible>) -> Self {
        match e {
            TryEncodeError::Encode(e) => e,
        }
    }
}

/// Tries to write an iterator of [`Result`]s as a MessagePack array.
///
/// The difference from [`write_array`] is that this function accepts an iterator of [`Result`]s
/// rather than an iterator of elements. If any of the [`Result`]s is an [`Err`], this function will
/// return [`TryEncodeError::Inner`] with that error.
///
/// `write` is a function that writes a single element.
pub fn try_write_array<W, S, T, E, F, WrErr>(
    writer: &mut W,
    iter: S,
    write: F,
) -> Result<(), TryEncodeError<W::Error, E>>
where
    W: RmpWrite,
    S: IntoIterator<IntoIter: ExactSizeIterator<Item = Result<T, E>>>,
    F: FnMut(&mut W, T) -> Result<(), WrErr>,
    WrErr: Into<EncodeError<W::Error>>,
{
    try_write_array_impl(writer, iter, write_array_len, write)
}

/// Helper function for sharing code between [`try_write_array`] and [`write_binary_array`].
fn try_write_array_impl<W, S, T, E, L, F, WrErr>(
    writer: &mut W,
    iter: S,
    write_len: L,
    mut write: F,
) -> Result<(), TryEncodeError<W::Error, E>>
where
    W: RmpWrite,
    // Ideally we would require `TrustedLen`, but that's unstable.
    S: IntoIterator<IntoIter: ExactSizeIterator<Item = Result<T, E>>>,
    L: FnOnce(&mut W, u32) -> Result<Marker, ValueWriteError<W::Error>>,
    F: FnMut(&mut W, T) -> Result<(), WrErr>,
    WrErr: Into<EncodeError<W::Error>>,
{
    let mut iter = iter.into_iter();
    let len = u32::try_from(iter.len()).map_err(|_| EncodeError::ArrayOverflow)?;
    write_len(writer, len).map_err(EncodeError::from)?;

    // The "incorrect implementation of ExactSizeIterator" panics are not expected to happen in
    // practice. To trigger them, we would have to write a custom implementation of
    // `ExactSizeIterator` for a type, get it wrong, and then call this function on that custom
    // iterator. The implementations of `ExactSizeIterator` for all the standard types like slices
    // and `Vec`s are known to be correct and cannot trigger this. There is existing precedent for
    // panicking when the programmer has failed to implement a trait in the correct way; for
    // example, `std::collections::HashMap` will panic if `Eq` and `Hash` do not agree.

    let mut count: u32 = 0;
    iter.try_for_each(|result| {
        count = count
            .checked_add(1)
            .expect("programming error: incorrect implementation of ExactSizeIterator");
        let item = result.map_err(TryEncodeError::Inner)?;
        write(writer, item).map_err(|e| TryEncodeError::from(e.into()))
    })?;

    assert_eq!(
        len, count,
        "programming error: incorrect implementation of ExactSizeIterator"
    );
    Ok(())
}
