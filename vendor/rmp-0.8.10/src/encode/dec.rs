use std::io::Write;

use crate::Marker;
use crate::encode::ValueWriteError;
use super::{write_marker, write_data_f32, write_data_f64};

/// Encodes and attempts to write an `f32` value as a 5-byte sequence into the given write.
///
/// The first byte becomes the `f32` marker and the others will represent the data itself.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_f32<W: Write>(wr: &mut W, val: f32) -> Result<(), ValueWriteError> {
    write_marker(wr, Marker::F32)?;
    write_data_f32(wr, val)?;
    Ok(())
}

/// Encodes and attempts to write an `f64` value as a 9-byte sequence into the given write.
///
/// The first byte becomes the `f64` marker and the others will represent the data itself.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_f64<W: Write>(wr: &mut W, val: f64) -> Result<(), ValueWriteError> {
    write_marker(wr, Marker::F64)?;
    write_data_f64(wr, val)?;
    Ok(())
}
