use std::io::Write;

use crate::Marker;
use crate::encode::ValueWriteError;
use super::{write_marker, write_data_u8, write_data_u16, write_data_u32};

/// Encodes and attempts to write the most efficient string length implementation to the given
/// write, returning the marker used.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_str_len<W: Write>(wr: &mut W, len: u32) -> Result<Marker, ValueWriteError> {
    if len < 32 {
        write_marker(wr, Marker::FixStr(len as u8))?;
        Ok(Marker::FixStr(len as u8))
    } else if len < 256 {
        write_marker(wr, Marker::Str8)?;
        write_data_u8(wr, len as u8)?;
        Ok(Marker::Str8)
    } else if len < 65536 {
        write_marker(wr, Marker::Str16)?;
        write_data_u16(wr, len as u16)?;
        Ok(Marker::Str16)
    } else {
        write_marker(wr, Marker::Str32)?;
        write_data_u32(wr, len)?;
        Ok(Marker::Str32)
    }
}

/// Encodes and attempts to write the most efficient string binary representation to the
/// given `Write`.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
// TODO: Docs, range check, example, visibility.
pub fn write_str<W: Write>(wr: &mut W, data: &str) -> Result<(), ValueWriteError> {
    write_str_len(wr, data.len() as u32)?;
    wr.write_all(data.as_bytes()).map_err(ValueWriteError::InvalidDataWrite)
}
