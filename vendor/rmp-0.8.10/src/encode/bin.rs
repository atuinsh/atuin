use std::io::Write;

use crate::Marker;
use crate::encode::{write_marker, ValueWriteError};
use super::{write_data_u8, write_data_u16, write_data_u32};

/// Encodes and attempts to write the most efficient binary array length implementation to the given
/// write, returning the marker used.
///
/// This function is useful when you want to get full control for writing the data itself, for
/// example, when using non-blocking socket.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
pub fn write_bin_len<W: Write>(wr: &mut W, len: u32) -> Result<Marker, ValueWriteError> {
    if len < 256 {
        write_marker(wr, Marker::Bin8)?;
        write_data_u8(wr, len as u8)?;
        Ok(Marker::Bin8)
    } else if len < 65536 {
        write_marker(wr, Marker::Bin16)?;
        write_data_u16(wr, len as u16)?;
        Ok(Marker::Bin16)
    } else {
        write_marker(wr, Marker::Bin32)?;
        write_data_u32(wr, len)?;
        Ok(Marker::Bin32)
    }
}

/// Encodes and attempts to write the most efficient binary implementation to the given `Write`.
///
/// # Errors
///
/// This function will return `ValueWriteError` on any I/O error occurred while writing either the
/// marker or the data.
// TODO: Docs, range check, example, visibility.
pub fn write_bin<W: Write>(wr: &mut W, data: &[u8]) -> Result<(), ValueWriteError> {
    write_bin_len(wr, data.len() as u32)?;
    wr.write_all(data).map_err(ValueWriteError::InvalidDataWrite)
}
