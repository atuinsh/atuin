use rmp::Marker;
pub use rmp::decode::bytes::{Bytes, BytesReadError};
pub use rmp::decode::{
    DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError, read_array_len,
    read_bin_len, read_bool, read_i8, read_i16, read_i32, read_i64, read_int, read_map_len,
    read_str_from_slice, read_str_len, read_u8, read_u16, read_u32, read_u64,
};

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
    #[display("{_0}")]
    Custom(String),
}

impl<E: RmpReadErr> DecodeError<'_, E> {
    pub fn into_static(self) -> DecodeError<'static, E> {
        match self {
            Self::DecodeString(e) => DecodeError::DecodeString(match e {
                DecodeStringError::InvalidMarkerRead(e) => DecodeStringError::InvalidMarkerRead(e),
                DecodeStringError::InvalidDataRead(e) => DecodeStringError::InvalidDataRead(e),
                DecodeStringError::TypeMismatch(m) => DecodeStringError::TypeMismatch(m),
                DecodeStringError::BufferSizeTooSmall(n) => {
                    DecodeStringError::BufferSizeTooSmall(n)
                }
                DecodeStringError::InvalidUtf8(_, e) => {
                    DecodeStringError::InvalidUtf8(b"[elided]", e)
                }
            }),
            Self::NumValueRead(e) => DecodeError::NumValueRead(e),
            Self::ValueRead(e) => DecodeError::ValueRead(e),
            Self::WrongArrayLength { expected, actual } => {
                DecodeError::WrongArrayLength { expected, actual }
            }
            Self::Custom(s) => DecodeError::Custom(s),
        }
    }

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
pub fn read_string<'a>(bytes: &mut Bytes<'a>) -> Result<String, DecodeError<'a>> {
    let slice = bytes.remaining_slice();
    let (string, rest) = match read_str_from_slice(slice) {
        Ok(pair) => pair,
        Err(e) => {
            if let DecodeStringError::TypeMismatch(_) = e {
                // The decode functions in `rmp::decode` consume the marker byte when there's a
                // type mismatch; make sure we do that too, as `read_optional` depends on it.
                bytes.read_u8().expect("TypeMismatch implies stream contains a marker byte");
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
/// also more convenient than [`read_str_from_slice`] because you don't have to unwrap and re-create
/// the [`Bytes`] object.
pub fn with_str<'a, F, R>(bytes: &mut Bytes<'a>, f: F) -> Result<R, DecodeError<'a>>
where
    F: FnOnce(&'a str) -> R,
{
    let slice = bytes.remaining_slice();
    let (string, rest) = read_str_from_slice(slice)?;
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
    reader: &mut R,
    read: F,
) -> Result<Option<T>, DecodeError<'a, R::Error>>
where
    R: RmpRead,
    F: FnOnce(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    let err = match read(reader) {
        Ok(v) => return Ok(Some(v)),
        Err(e) => e.into(),
    };

    if let Some(Marker::Null) = err.type_mismatch() {
        Ok(None)
    } else {
        Err(err)
    }
}

/// Read a MessagePack array as an iterator of elements.
///
/// `read` is a function that reads a single element.
pub fn read_array<'a, R, T, F, E>(
    reader: &mut R,
    read: F,
) -> impl Iterator<Item = Result<T, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    read_array_impl(reader, read_array_len, read)
}

/// Like [`read_array`], but reads into a fixed-size Rust array.
///
/// An error is returned if the actual array length does not equal `N`.
pub fn read_fixed_array<'a, const N: usize, R, T, F, E>(
    reader: &mut R,
    read: F,
) -> Result<[T; N], DecodeError<'a, R::Error>>
where
    R: RmpRead,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    read_fixed_array_impl(reader, read_array_len, read)
}

/// Read a MessagePack binary array as an iterator of bytes.
pub fn read_binary_array<'a, R>(
    reader: &mut R,
) -> impl Iterator<Item = Result<u8, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
{
    read_array_impl(reader, read_bin_len, |reader| {
        reader.read_u8().map_err(ValueReadError::InvalidDataRead)
    })
}

/// Read a MessagePack binary array into a fixed-size Rust array of bytes.
///
/// An error is returned if the actual array length does not equal `N`.
pub fn read_fixed_binary_array<'a, const N: usize, R>(
    reader: &mut R,
) -> Result<[u8; N], DecodeError<'a, R::Error>>
where
    R: RmpRead,
{
    read_fixed_array_impl(reader, read_bin_len, |reader| {
        reader.read_u8().map_err(ValueReadError::InvalidDataRead)
    })
}

/// Helper function for sharing code between [`read_array`] and [`read_binary_array`].
fn read_array_impl<'a, R, T, L, F, E>(
    reader: &mut R,
    read_len: L,
    mut read: F,
) -> impl Iterator<Item = Result<T, DecodeError<'a, R::Error>>>
where
    R: RmpRead,
    L: FnOnce(&mut R) -> Result<u32, ValueReadError<R::Error>>,
    F: FnMut(&mut R) -> Result<T, E>,
    E: Into<DecodeError<'a, R::Error>>,
{
    let (len, error) = match read_len(reader) {
        Ok(len) => (len, None),
        Err(e) => (0, Some(e)),
    };

    let items = (0..len).map(move |_| read(reader).map_err(Into::into));
    // Errors from `read_array_len` are yielded as an element of the iterator. Otherwise we would
    // have wrap this function's return type in another `Result`, which is less ergonomic.
    error.into_iter().map(|e| Err(e.into())).chain(items)
}

/// Helper function for sharing code between [`read_fixed_array`] and [`read_fixed_binary_array`].
fn read_fixed_array_impl<'a, const N: usize, R, T, L, F, E>(
    reader: &mut R,
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

    let actual_len = read_len(reader)?;
    if !u32::try_from(N).is_ok_and(|n| n == actual_len) {
        return Err(DecodeError::WrongArrayLength {
            expected: N,
            actual: actual_len,
        });
    }

    let mut array = MaybeUninit::<[T; N]>::uninit();
    let array_of_uninit: &mut [MaybeUninit<T>; N] = array.as_mut();
    for slot in array_of_uninit {
        slot.write(read(reader).map_err(Into::into)?);
    }

    #[allow(
        unsafe_code,
        reason = "Doing this without unsafe code is much less efficient. We would have to create \
                  an
            `[Option<T>; N]`, which could be up to twice the size, fill in each element, and then
            use `array::map` to call `Option::unwrap` on each element. Besides the overhead of
            `unwrap`, `array::map` is noted as being inefficient on large arrays."
    )]
    // SAFETY: We fully initialized the array by calling `MaybeUninit::write` on every element of
    // the array.
    Ok(unsafe { array.assume_init() })
}
