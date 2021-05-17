//! Provides various functions and structs for MessagePack encoding.

mod bin;
mod dec;
mod ext;
mod map;
mod sint;
mod str;
mod uint;
mod vec;

pub use self::sint::{write_nfix, write_i8, write_i16, write_i32, write_i64, write_sint};
pub use self::uint::{write_pfix, write_u8, write_u16, write_u32, write_u64, write_uint};
pub use self::dec::{write_f32, write_f64};
pub use self::str::{write_str_len, write_str};
pub use self::bin::{write_bin_len, write_bin};

use std::error;
use std::fmt::{self, Display, Formatter};
use std::io::Write;

use byteorder::{self, WriteBytesExt};

use crate::Marker;

/// The error type for I/O operations of the `Write` and associated traits.
pub type Error = ::std::io::Error;

// An error returned from the `write_marker` and `write_fixval` functions.
struct MarkerWriteError(Error);

impl From<Error> for MarkerWriteError {
    #[cold]
    fn from(err: Error) -> MarkerWriteError {
        MarkerWriteError(err)
    }
}

impl From<MarkerWriteError> for Error {
    #[cold]
    fn from(err: MarkerWriteError) -> Error {
        match err {
            MarkerWriteError(err) => err
        }
    }
}

/// Attempts to write the given marker into the writer.
fn write_marker<W: Write>(wr: &mut W, marker: Marker) -> Result<(), MarkerWriteError> {
    wr.write_u8(marker.to_u8()).map_err(MarkerWriteError)
}

/// An error returned from primitive values write functions.
struct DataWriteError(Error);

impl From<Error> for DataWriteError {
    #[cold]
    #[inline]
    fn from(err: Error) -> DataWriteError {
        DataWriteError(err)
    }
}

impl From<DataWriteError> for Error {
    #[cold]
    #[inline]
    fn from(err: DataWriteError) -> Error {
        err.0
    }
}

/// Encodes and attempts to write a nil value into the given write.
///
/// According to the MessagePack specification, a nil value is represented as a single `0xc0` byte.
///
/// # Errors
///
/// This function will return `Error` on any I/O error occurred while writing the nil marker.
///
/// # Examples
///
/// ```
/// let mut buf = Vec::new();
///
/// rmp::encode::write_nil(&mut buf).unwrap();
///
/// assert_eq!(vec![0xc0], buf);
/// ```
#[inline]
pub fn write_nil<W: Write>(wr: &mut W) -> Result<(), Error> {
    write_marker(wr, Marker::Null).map_err(From::from)
}

/// Encodes and attempts to write a bool value into the given write.
///
/// According to the MessagePack specification, an encoded boolean value is represented as a single
/// byte.
///
/// # Errors
///
/// Each call to this function may generate an I/O error indicating that the operation could not be
/// completed.
#[inline]
pub fn write_bool<W: Write>(wr: &mut W, val: bool) -> Result<(), Error> {
    let marker = if val {
        Marker::True
    } else {
        Marker::False
    };

    write_marker(wr, marker).map_err(From::from)
}

#[inline]
fn write_data_u8<W: Write>(wr: &mut W, val: u8) -> Result<(), DataWriteError> {
    wr.write_u8(val).map_err(DataWriteError)
}

#[inline]
fn write_data_u16<W: Write>(wr: &mut W, val: u16) -> Result<(), DataWriteError> {
    wr.write_u16::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_u32<W: Write>(wr: &mut W, val: u32) -> Result<(), DataWriteError> {
    wr.write_u32::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_u64<W: Write>(wr: &mut W, val: u64) -> Result<(), DataWriteError> {
    wr.write_u64::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_i8<W: Write>(wr: &mut W, val: i8) -> Result<(), DataWriteError> {
    wr.write_i8(val).map_err(DataWriteError)
}

#[inline]
fn write_data_i16<W: Write>(wr: &mut W, val: i16) -> Result<(), DataWriteError> {
    wr.write_i16::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_i32<W: Write>(wr: &mut W, val: i32) -> Result<(), DataWriteError> {
    wr.write_i32::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_i64<W: Write>(wr: &mut W, val: i64) -> Result<(), DataWriteError> {
    wr.write_i64::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_f32<W: Write>(wr: &mut W, val: f32) -> Result<(), DataWriteError> {
    wr.write_f32::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

#[inline]
fn write_data_f64<W: Write>(wr: &mut W, val: f64) -> Result<(), DataWriteError> {
    wr.write_f64::<byteorder::BigEndian>(val).map_err(DataWriteError)
}

/// An error that can occur when attempting to write multi-byte MessagePack value.
#[derive(Debug)]
pub enum ValueWriteError {
    /// I/O error while writing marker.
    InvalidMarkerWrite(Error),
    /// I/O error while writing data.
    InvalidDataWrite(Error),
}

impl From<MarkerWriteError> for ValueWriteError {
    #[cold]
    fn from(err: MarkerWriteError) -> ValueWriteError {
        match err {
            MarkerWriteError(err) => ValueWriteError::InvalidMarkerWrite(err),
        }
    }
}

impl From<DataWriteError> for ValueWriteError {
    #[cold]
    fn from(err: DataWriteError) -> ValueWriteError {
        match err {
            DataWriteError(err) => ValueWriteError::InvalidDataWrite(err),
        }
    }
}

impl From<ValueWriteError> for Error {
    #[cold]
    fn from(err: ValueWriteError) -> Error {
        match err {
            ValueWriteError::InvalidMarkerWrite(err) |
            ValueWriteError::InvalidDataWrite(err) => err,
        }
    }
}

impl error::Error for ValueWriteError {
    #[cold]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            ValueWriteError::InvalidMarkerWrite(ref err) |
            ValueWriteError::InvalidDataWrite(ref err) => Some(err),
        }
    }
}

impl Display for ValueWriteError {
    #[cold]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("error while writing multi-byte MessagePack value")
    }
}

/// Encodes and attempts to write the most efficient array length implementation to the given write,
/// returning the marker used.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_array_len<W: Write>(wr: &mut W, len: u32) -> Result<Marker, ValueWriteError> {
    let marker = if len < 16 {
        write_marker(wr, Marker::FixArray(len as u8))?;
        Marker::FixArray(len as u8)
    } else if len < 65536 {
        write_marker(wr, Marker::Array16)?;
        write_data_u16(wr, len as u16)?;
        Marker::Array16
    } else {
        write_marker(wr, Marker::Array32)?;
        write_data_u32(wr, len)?;
        Marker::Array32
    };

    Ok(marker)
}

/// Encodes and attempts to write the most efficient map length implementation to the given write,
/// returning the marker used.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_map_len<W: Write>(wr: &mut W, len: u32) -> Result<Marker, ValueWriteError> {
    let marker = if len < 16 {
        write_marker(wr, Marker::FixMap(len as u8))?;
        Marker::FixMap(len as u8)
    } else if len < 65536 {
        write_marker(wr, Marker::Map16)?;
        write_data_u16(wr, len as u16)?;
        Marker::Map16
    } else {
        write_marker(wr, Marker::Map32)?;
        write_data_u32(wr, len)?;
        Marker::Map32
    };

    Ok(marker)
}

/// Encodes and attempts to write the most efficient ext metadata implementation to the given
/// write, returning the marker used.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
///
/// # Panics
///
/// Panics if `ty` is negative, because it is reserved for future MessagePack extension including
/// 2-byte type information.
pub fn write_ext_meta<W: Write>(wr: &mut W, len: u32, ty: i8) -> Result<Marker, ValueWriteError> {
    let marker = match len {
        1 => {
            write_marker(wr, Marker::FixExt1)?;
            Marker::FixExt1
        }
        2 => {
            write_marker(wr, Marker::FixExt2)?;
            Marker::FixExt2
        }
        4 => {
            write_marker(wr, Marker::FixExt4)?;
            Marker::FixExt4
        }
        8 => {
            write_marker(wr, Marker::FixExt8)?;
            Marker::FixExt8
        }
        16 => {
            write_marker(wr, Marker::FixExt16)?;
            Marker::FixExt16
        }
        len if len < 256 => {
            write_marker(wr, Marker::Ext8)?;
            write_data_u8(wr, len as u8)?;
            Marker::Ext8
        }
        len if len < 65536 => {
            write_marker(wr, Marker::Ext16)?;
            write_data_u16(wr, len as u16)?;
            Marker::Ext16
        }
        len => {
            write_marker(wr, Marker::Ext32)?;
            write_data_u32(wr, len)?;
            Marker::Ext32
        }
    };

    write_data_i8(wr, ty)?;

    Ok(marker)
}
