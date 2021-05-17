//! Provides various functions and structs for MessagePack decoding.
//!
//! Most of the function defined in this module will silently handle interruption error (EINTR)
//! received from the given `Read` to be in consistent state with the `Write::write_all` method in
//! the standard library.
//!
//! Any other error would immediately interrupt the parsing process. If your reader can results in
//! I/O error and simultaneously be a recoverable state (for example, when reading from
//! non-blocking socket and it returns EWOULDBLOCK) be sure that you buffer the data externally
//! to avoid data loss (using `BufRead` readers with manual consuming or some other way).

mod dec;
mod ext;
mod sint;
mod str;
mod uint;

pub use self::sint::{read_nfix, read_i8, read_i16, read_i32, read_i64};
pub use self::uint::{read_pfix, read_u8, read_u16, read_u32, read_u64};
pub use self::dec::{read_f32, read_f64};
#[allow(deprecated)] // While we re-export deprecated items, we don't want to trigger warnings while compiling this crate
pub use self::str::{read_str_len, read_str, read_str_from_slice, read_str_ref, DecodeStringError};
pub use self::ext::{read_fixext1, read_fixext2, read_fixext4, read_fixext8, read_fixext16, read_ext_meta, ExtMeta};

use std::error;
use std::fmt::{self, Display, Formatter};
use std::io::Read;

use byteorder::{self, ReadBytesExt};

use num_traits::cast::FromPrimitive;

use crate::Marker;

/// An error that can occur when attempting to read bytes from the reader.
pub type Error = ::std::io::Error;

/// An error that can occur when attempting to read a MessagePack marker from the reader.
#[derive(Debug)]
pub struct MarkerReadError(pub Error);

/// An error which can occur when attempting to read a MessagePack value from the reader.
#[derive(Debug)]
pub enum ValueReadError {
    /// Failed to read the marker.
    InvalidMarkerRead(Error),
    /// Failed to read the data.
    InvalidDataRead(Error),
    /// The type decoded isn't match with the expected one.
    TypeMismatch(Marker),
}

impl error::Error for ValueReadError {
    #[cold]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            ValueReadError::InvalidMarkerRead(ref err) |
            ValueReadError::InvalidDataRead(ref err) => Some(err),
            ValueReadError::TypeMismatch(..) => None,
        }
    }
}

impl Display for ValueReadError {
    #[cold]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str(match *self {
            ValueReadError::InvalidMarkerRead(..) => "failed to read MessagePack marker",
            ValueReadError::InvalidDataRead(..) => "failed to read MessagePack data",
            ValueReadError::TypeMismatch(..) => {
                "the type decoded isn't match with the expected one"
            }
        })
    }
}

impl From<MarkerReadError> for ValueReadError {
    #[cold]
    fn from(err: MarkerReadError) -> ValueReadError {
        match err {
            MarkerReadError(err) => ValueReadError::InvalidMarkerRead(err),
        }
    }
}

impl From<Error> for MarkerReadError {
    #[cold]
    fn from(err: Error) -> MarkerReadError {
        MarkerReadError(err)
    }
}

/// Attempts to read a single byte from the given reader and to decode it as a MessagePack marker.
#[inline]
pub fn read_marker<R: Read>(rd: &mut R) -> Result<Marker, MarkerReadError> {
    Ok(Marker::from_u8(rd.read_u8()?))
}

/// Attempts to read a single byte from the given reader and to decode it as a nil value.
///
/// According to the MessagePack specification, a nil value is represented as a single `0xc0` byte.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading the nil marker,
/// except the EINTR, which is handled internally.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_nil<R: Read>(rd: &mut R) -> Result<(), ValueReadError> {
    match read_marker(rd)? {
        Marker::Null => Ok(()),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read a single byte from the given reader and to decode it as a boolean value.
///
/// According to the MessagePack specification, an encoded boolean value is represented as a single
/// byte.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading the bool marker,
/// except the EINTR, which is handled internally.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_bool<R: Read>(rd: &mut R) -> Result<bool, ValueReadError> {
    match read_marker(rd)? {
        Marker::True => Ok(true),
        Marker::False => Ok(false),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// An error which can occur when attempting to read a MessagePack numeric value from the reader.
#[derive(Debug)]
pub enum NumValueReadError {
    /// Failed to read the marker.
    InvalidMarkerRead(Error),
    /// Failed to read the data.
    InvalidDataRead(Error),
    /// The type decoded isn't match with the expected one.
    TypeMismatch(Marker),
    /// Out of range integral type conversion attempted.
    OutOfRange,
}

impl error::Error for NumValueReadError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            NumValueReadError::InvalidMarkerRead(ref err) |
            NumValueReadError::InvalidDataRead(ref err) => Some(err),
            NumValueReadError::TypeMismatch(..) |
            NumValueReadError::OutOfRange => None,
        }
    }
}

impl Display for NumValueReadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str(match *self {
            NumValueReadError::InvalidMarkerRead(..) => "failed to read MessagePack marker",
            NumValueReadError::InvalidDataRead(..) => "failed to read MessagePack data",
            NumValueReadError::TypeMismatch(..) => {
                "the type decoded isn't match with the expected one"
            }
            NumValueReadError::OutOfRange => "out of range integral type conversion attempted",
        })
    }
}

impl From<MarkerReadError> for NumValueReadError {
    #[cold]
    fn from(err: MarkerReadError) -> NumValueReadError {
        match err {
            MarkerReadError(err) => NumValueReadError::InvalidMarkerRead(err),
        }
    }
}

impl From<ValueReadError> for NumValueReadError {
    #[cold]
    fn from(err: ValueReadError) -> NumValueReadError {
        match err {
            ValueReadError::InvalidMarkerRead(err) => NumValueReadError::InvalidMarkerRead(err),
            ValueReadError::InvalidDataRead(err) => NumValueReadError::InvalidDataRead(err),
            ValueReadError::TypeMismatch(err) => NumValueReadError::TypeMismatch(err),
        }
    }
}

// Helper functions to map I/O error into the `InvalidDataRead` error.

#[doc(hidden)]
#[inline]
pub fn read_data_u8<R: Read>(rd: &mut R) -> Result<u8, ValueReadError> {
    rd.read_u8().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_u16<R: Read>(rd: &mut R) -> Result<u16, ValueReadError> {
    rd.read_u16::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_u32<R: Read>(rd: &mut R) -> Result<u32, ValueReadError> {
    rd.read_u32::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_u64<R: Read>(rd: &mut R) -> Result<u64, ValueReadError> {
    rd.read_u64::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_i8<R: Read>(rd: &mut R) -> Result<i8, ValueReadError> {
    rd.read_i8().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_i16<R: Read>(rd: &mut R) -> Result<i16, ValueReadError> {
    rd.read_i16::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_i32<R: Read>(rd: &mut R) -> Result<i32, ValueReadError> {
    rd.read_i32::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_i64<R: Read>(rd: &mut R) -> Result<i64, ValueReadError> {
    rd.read_i64::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_f32<R: Read>(rd: &mut R) -> Result<f32, ValueReadError> {
    rd.read_f32::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

#[doc(hidden)]
#[inline]
pub fn read_data_f64<R: Read>(rd: &mut R) -> Result<f64, ValueReadError> {
    rd.read_f64::<byteorder::BigEndian>().map_err(ValueReadError::InvalidDataRead)
}

/// Attempts to read up to 9 bytes from the given reader and to decode them as integral `T` value.
///
/// This function will try to read up to 9 bytes from the reader (1 for marker and up to 8 for data)
/// and interpret them as a big-endian `T`.
///
/// Unlike `read_*`, this function weakens type restrictions, allowing you to safely decode packed
/// values even if you aren't sure about the actual integral type.
///
/// # Errors
///
/// This function will return `NumValueReadError` on any I/O error while reading either the marker
/// or the data.
///
/// It also returns `NumValueReadError::OutOfRange` if the actual type is not an integer or it does
/// not fit in the given numeric range.
///
/// # Examples
///
/// ```
/// let buf = [0xcd, 0x1, 0x2c];
///
/// assert_eq!(300u16, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300i16, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300u32, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300i32, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300u64, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300i64, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300usize, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// assert_eq!(300isize, rmp::decode::read_int(&mut &buf[..]).unwrap());
/// ```
pub fn read_int<T: FromPrimitive, R: Read>(rd: &mut R) -> Result<T, NumValueReadError> {
    let val = match read_marker(rd)? {
        Marker::FixPos(val) => T::from_u8(val),
        Marker::FixNeg(val) => T::from_i8(val),
        Marker::U8 => T::from_u8(read_data_u8(rd)?),
        Marker::U16 => T::from_u16(read_data_u16(rd)?),
        Marker::U32 => T::from_u32(read_data_u32(rd)?),
        Marker::U64 => T::from_u64(read_data_u64(rd)?),
        Marker::I8 => T::from_i8(read_data_i8(rd)?),
        Marker::I16 => T::from_i16(read_data_i16(rd)?),
        Marker::I32 => T::from_i32(read_data_i32(rd)?),
        Marker::I64 => T::from_i64(read_data_i64(rd)?),
        marker => return Err(NumValueReadError::TypeMismatch(marker)),
    };

    val.ok_or(NumValueReadError::OutOfRange)
}

/// Attempts to read up to 5 bytes from the given reader and to decode them as a big-endian u32
/// array size.
///
/// Array format family stores a sequence of elements in 1, 3, or 5 bytes of extra bytes in addition
/// to the elements.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
// TODO: Docs.
// NOTE: EINTR is managed internally.
pub fn read_array_len<R>(rd: &mut R) -> Result<u32, ValueReadError>
where
    R: Read,
{
    match read_marker(rd)? {
        Marker::FixArray(size) => Ok(size as u32),
        Marker::Array16 => Ok(read_data_u16(rd)? as u32),
        Marker::Array32 => Ok(read_data_u32(rd)?),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read up to 5 bytes from the given reader and to decode them as a big-endian u32
/// map size.
///
/// Map format family stores a sequence of elements in 1, 3, or 5 bytes of extra bytes in addition
/// to the elements.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
// TODO: Docs.
pub fn read_map_len<R: Read>(rd: &mut R) -> Result<u32, ValueReadError> {
    let marker = read_marker(rd)?;
    marker_to_len(rd, marker)
}

pub fn marker_to_len<R: Read>(rd: &mut R, marker: Marker) -> Result<u32, ValueReadError> {
    match marker {
        Marker::FixMap(size) => Ok(size as u32),
        Marker::Map16 => Ok(read_data_u16(rd)? as u32),
        Marker::Map32 => Ok(read_data_u32(rd)?),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read up to 5 bytes from the given reader and to decode them as Binary array length.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
// TODO: Docs.
pub fn read_bin_len<R: Read>(rd: &mut R) -> Result<u32, ValueReadError> {
    match read_marker(rd)? {
        Marker::Bin8 => Ok(read_data_u8(rd)? as u32),
        Marker::Bin16 => Ok(read_data_u16(rd)? as u32),
        Marker::Bin32 => Ok(read_data_u32(rd)?),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}
