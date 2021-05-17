use std::io::Read;

use crate::Marker;
use super::{read_marker, read_data_i8, read_data_i16, read_data_i32, read_data_i64, ValueReadError};

/// Attempts to read a single byte from the given reader and to decode it as a negative fixnum
/// value.
///
/// According to the MessagePack specification, a negative fixed integer value is represented using
/// a single byte in `[0xe0; 0xff]` range inclusively, prepended with a special marker mask.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading the marker,
/// except the EINTR, which is handled internally.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_nfix<R: Read>(rd: &mut R) -> Result<i8, ValueReadError> {
    match read_marker(rd)? {
        Marker::FixNeg(val) => Ok(val),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 2 bytes from the given reader and to decode them as `i8` value.
///
/// The first byte should be the marker and the second one should represent the data itself.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_i8<R: Read>(rd: &mut R) -> Result<i8, ValueReadError> {
    match read_marker(rd)? {
        Marker::I8 => read_data_i8(rd),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 3 bytes from the given reader and to decode them as `i16` value.
///
/// The first byte should be the marker and the others should represent the data itself.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_i16<R: Read>(rd: &mut R) -> Result<i16, ValueReadError> {
    match read_marker(rd)? {
        Marker::I16 => read_data_i16(rd),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 5 bytes from the given reader and to decode them as `i32` value.
///
/// The first byte should be the marker and the others should represent the data itself.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_i32<R: Read>(rd: &mut R) -> Result<i32, ValueReadError> {
    match read_marker(rd)? {
        Marker::I32 => read_data_i32(rd),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 9 bytes from the given reader and to decode them as `i64` value.
///
/// The first byte should be the marker and the others should represent the data itself.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
///
/// It also returns `ValueReadError::TypeMismatch` if the actual type is not equal with the
/// expected one, indicating you with the actual type.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_i64<R: Read>(rd: &mut R) -> Result<i64, ValueReadError> {
    match read_marker(rd)? {
        Marker::I64 => read_data_i64(rd),
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}
