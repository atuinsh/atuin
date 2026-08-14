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
#[derive(Debug, thiserror::Error)]
pub enum EncodeError<E: RmpWriteErr = std::io::Error> {
    #[error("could not write MessagePack value: {0:?}")]
    ValueWrite(#[from] ValueWriteError<E>),
    #[error("cannot encode array larger than {}", u32::MAX)]
    ArrayOverflow,
}

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
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display, derive_more::From)]
#[display("could not decode MessagePack value: {_0:?}")]
pub enum DecodeError<'a, E: RmpReadErr = BytesReadError> {
    DecodeString(DecodeStringError<'a, E>),
    NumValueRead(NumValueReadError<E>),
    ValueRead(ValueReadError<E>),
    #[display("expected array of length {expected}, but got {actual}")]
    WrongArrayLength {
        expected: usize,
        actual: u32,
    },
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

/// Reads a string from a [`Bytes`] object and calls a function with that string.
///
/// If you don't need an owned string, this function is more efficient than [`read_string`]. It is
/// also more convenient than [`decode::read_str_from_slice`] because you don't have to unwrap and
/// re-create the [`Bytes`] object.
pub fn with_str<'a, F, R>(bytes: &mut Bytes<'a>, f: F) -> Result<R, DecodeError<'a>>
where
    F: FnOnce(&'a str) -> R,
{
    let slice = bytes.remaining_slice();
    let (string, rest) = decode::read_str_from_slice(slice)?;
    let result = f(string);
    *bytes = Bytes::new(rest);
    Ok(result)
}

/// Read an optional value from the stream.
///
/// This function calls `read`, which should try to decode a value of type `T` from the stream. If
/// that function returns an error indicating [`Marker::Null`] was encountered instead, this
/// function returns [`None`]. All other errors are forwarded as-is.
pub fn read_optional<'a, R, T, F, E>(
    input: &mut R,
    read: F,
) -> Result<Option<T>, DecodeError<'a, R::Error>>
where
    R: RmpRead,
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
///
/// Because of the way this function works, the first byte written by `write` must *never* be
/// [`Marker::Null`]. That would introduce ambiguity as to whether `value` was [`None`], or `value`
/// was [`Some`] but began with a null marker.
pub fn write_optional<W, T, F, E>(
    output: &mut W,
    value: Option<T>,
    write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    F: FnOnce(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    match value {
        Some(v) => write(output, v).map_err(Into::into),
        None => {
            encode::write_nil(output).map_err(|e| ValueWriteError::InvalidMarkerWrite(e).into())
        }
    }
}

/// Read a MessagePack array as an iterator of elements.
///
/// `read` is a function that reads a single element.
pub fn read_array<'a, R, T, F, E>(
    input: &mut R,
    read: F,
) -> impl Iterator<Item = Result<T, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    read_array_impl(input, decode::read_array_len, read)
}

/// Like [`read_array`], but reads into a fixed-size Rust array.
///
/// An error is returned if the actual array length does not equal `N`.
pub fn read_fixed_array<'a, const N: usize, R, T, F, E>(
    input: &mut R,
    mut read: F,
) -> Result<[T; N], DecodeError<'a, R::Error>>
where
    R: RmpRead,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    read_fixed_array_impl(input, decode::read_array_len, read)
}

/// Write an iterator of elements as a MessagePack array.
///
/// `write` is a function that writes a single element.
pub fn write_array<'a, W, S, T, F, E>(
    output: &mut W,
    iter: S,
    write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    // Ideally we would require `TrustedLen`, but that's unstable.
    S: IntoIterator<IntoIter: ExactSizeIterator<Item = T>>,
    F: FnMut(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    write_array_impl(output, iter, encode::write_array_len, write)
}

/// Read a MessagePack binary array as an iterator of bytes.
pub fn read_binary_array<'a, R>(
    input: &mut R,
) -> impl Iterator<Item = Result<u8, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
{
    read_array_impl(input, decode::read_bin_len, |input| {
        input.read_u8().map_err(ValueReadError::InvalidDataRead)
    })
}

/// Read a MessagePack binary array into a fixed-size Rust array of bytes.
///
/// An error is returned if the actual array length does not equal `N`.
pub fn read_fixed_binary_array<'a, const N: usize, R>(
    input: &mut R,
) -> Result<[u8; N], DecodeError<'a, R::Error>>
where
    R: RmpRead,
{
    read_fixed_array_impl(input, decode::read_bin_len, |input| {
        input.read_u8().map_err(ValueReadError::InvalidDataRead)
    })
}

/// Write an iterator of bytes as a MessagePack binary array.
pub fn write_binary_array<W, B, E>(output: &mut W, bytes: B) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    B: IntoIterator<IntoIter: ExactSizeIterator<Item = u8>>,
{
    write_array_impl(output, bytes, encode::write_bin_len, |output, byte| {
        output
            .write_u8(byte)
            .map_err(ValueWriteError::InvalidDataWrite)
    })
}

/// Helper function for sharing code between [`read_array`] and [`read_binary_array`].
fn read_array_impl<'a, R, T, L, F, E>(
    input: &mut R,
    read_len: L,
    read: F,
) -> impl Iterator<Item = Result<T, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
    L: FnOnce(&mut R) -> Result<u32, ValueReadError<R::Error>>,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    let (len, error) = match read_len(input) {
        Ok(len) => (len, None),
        Err(e) => (0, Some(e)),
    };

    // We're using `zip` instead of `take(len)` because `len` is a `u32`, not a `usize`, and
    // conversion from `u32` to `usize` is fallible. In practice, it is only fallible on 16-bit
    // platforms, which we don't support, so in theory we could just `.unwrap()` and call it a day,
    // but the `zip` approach avoids having to do explicit error handling and should be just about
    // as efficient -- `size_hint` will still be specific and accurate.
    let items = read_raw_seq(input, read)
        .zip(0..len)
        .map(|(result, _i)| result);
    // Errors from `read_array_len` are yielded as an element of the iterator. Otherwise we would
    // have wrap this function's return type in another `Result`, which is less ergonomic.
    error.into_iter().map(|e| Err(e.into())).chain(items)
}

/// Helper function for sharing code between [`read_fixed_array`] and [`read_fixed_binary_array`].
fn read_fixed_array_impl<'a, const N: usize, R, T, L, F, E>(
    input: &mut R,
    read_len: L,
    mut read: F,
) -> Result<[T; N], DecodeError<'a, R::Error>>
where
    R: RmpRead,
    L: FnOnce(&mut R) -> Result<u32, ValueReadError<R::Error>>,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    use std::mem::MaybeUninit;

    let actual_len = read_len(input)?;
    if !u32::try_from(N).is_ok_and(|n| n == actual_len) {
        return Err(DecodeError::WrongArrayLength {
            expected: N,
            actual: actual_len,
        });
    }

    let mut array = MaybeUninit::<[T; N]>::uninit();
    let array_of_uninit: &mut [MaybeUninit<T>; N] = array.as_mut();
    for slot in array_of_uninit {
        slot.write(read(input).map_err(Into::into)?);
    }

    #[allow(
        unsafe_code,
        reason = "Doing this without unsafe code is much less efficient. We would \
        have to create an `[Option<T>; N]`, which could be up to twice the size, fill in each \
        element, and then use `array::map` to call `Option::unwrap` on each element. Besides the \
        overhead of `unwrap`, `array::map` is noted as being inefficient on large arrays."
    )]
    // SAFETY: We fully initialized the array by calling `MaybeUninit::write` on every element of
    // the array.
    Ok(unsafe { array.assume_init() })
}

/// Helper function for sharing code between [`write_array`] and [`write_binary_array`].
fn write_array_impl<'a, W, S, T, L, F, E>(
    output: &mut W,
    iter: S,
    write_len: L,
    write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    // Ideally we would require `TrustedLen`, but that's unstable.
    S: IntoIterator<IntoIter: ExactSizeIterator<Item = T>>,
    L: FnOnce(&mut W, u32) -> Result<Marker, ValueWriteError<W::Error>>,
    F: FnMut(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    let iter = iter.into_iter();
    let len = u32::try_from(iter.len()).map_err(|_| EncodeError::ArrayOverflow)?;
    write_len(output, len)?;

    // The "incorrect implementation of ExactSizeIterator" panics are not expected to happen in
    // practice. To trigger them, we would have to write a custom implementation of
    // `ExactSizeIterator` for a type and somehow get it wrong, and then call this function on that
    // custom iterator. The implementations of `ExactSizeIterator` for all the standard types like
    // slices and `Vec`s are known to be correct and cannot trigger this. There is existing
    // precedent for panicking when the programmer has failed to implement a trait in the correct
    // way; for example, `std::collections::HashMap` will panic if `Eq` and `Hash` do not agree.

    let mut count: u32 = 0;
    let iter = iter.inspect(|_| {
        count = count
            .checked_add(1)
            .expect("programming error: incorrect implementation of ExactSizeIterator");
    });

    write_raw_seq(output, iter, write)?;
    assert_eq!(
        len, count,
        "programming error: incorrect implementation of ExactSizeIterator"
    );
    Ok(())
}

/// Read a raw sequence of items. This does *not* read the sequence length!
///
/// The returned iterator will go on forever. It is the caller's responsibility to stop pulling
/// elements from it at the appropriate point!
fn read_raw_seq<'a, R, T, F, E>(
    input: &mut R,
    mut read: F,
) -> impl Iterator<Item = Result<T, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    std::iter::repeat_with(move || read(input).map_err(Into::into))
}

/// Write a raw sequence of items. This does *not* write the sequence length!
fn write_raw_seq<'a, W, S, T, F, E>(
    output: &mut W,
    sequence: S,
    mut write: F,
) -> Result<(), EncodeError<W::Error>>
where
    W: RmpWrite,
    S: IntoIterator<Item = T>,
    F: FnMut(&mut W, T) -> Result<(), E>,
    E: Into<EncodeError<W::Error>>,
{
    sequence
        .into_iter()
        .try_for_each(|item| write(output, item))
        .map_err(Into::into)
}
