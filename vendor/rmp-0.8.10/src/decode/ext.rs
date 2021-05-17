use std::io::Read;

use crate::Marker;
use super::{read_marker, read_data_i8, read_data_u8, read_data_u16, read_data_u32, ValueReadError};

/// Attempts to read exactly 3 bytes from the given reader and interpret them as a fixext1 type
/// with data attached.
///
/// According to the MessagePack specification, a fixext1 stores an integer and a byte array whose
/// length is 1 byte. Its marker byte is `0xd4`.
///
/// Note, that this function copies a byte array from the reader to the output `u8` variable.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
///
/// # Note
///
/// This function will silently retry on every EINTR received from the underlying `Read` until
/// successful read.
pub fn read_fixext1<R: Read>(rd: &mut R) -> Result<(i8, u8), ValueReadError> {
    match read_marker(rd)? {
        Marker::FixExt1 => {
            let ty = read_data_i8(rd)?;
            let data = read_data_u8(rd)?;
            Ok((ty, data))
        }
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 4 bytes from the given reader and interpret them as a fixext2 type
/// with data attached.
///
/// According to the MessagePack specification, a fixext2 stores an integer and a byte array whose
/// length is 2 bytes. Its marker byte is `0xd5`.
///
/// Note, that this function copies a byte array from the reader to the output buffer, which is
/// unlikely if you want zero-copy functionality.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
pub fn read_fixext2<R: Read>(rd: &mut R) -> Result<(i8, [u8; 2]), ValueReadError> {
    match read_marker(rd)? {
        Marker::FixExt2 => {
            let mut buf = [0; 2];
            read_fixext_data(rd, &mut buf).map(|ty| (ty, buf))
        }
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 6 bytes from the given reader and interpret them as a fixext4 type
/// with data attached.
///
/// According to the MessagePack specification, a fixext4 stores an integer and a byte array whose
/// length is 4 bytes. Its marker byte is `0xd6`.
///
/// Note, that this function copies a byte array from the reader to the output buffer, which is
/// unlikely if you want zero-copy functionality.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
pub fn read_fixext4<R: Read>(rd: &mut R) -> Result<(i8, [u8; 4]), ValueReadError> {
    match read_marker(rd)? {
        Marker::FixExt4 => {
            let mut buf = [0; 4];
            read_fixext_data(rd, &mut buf).map(|ty| (ty, buf))
        }
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 10 bytes from the given reader and interpret them as a fixext8 type
/// with data attached.
///
/// According to the MessagePack specification, a fixext8 stores an integer and a byte array whose
/// length is 8 bytes. Its marker byte is `0xd7`.
///
/// Note, that this function copies a byte array from the reader to the output buffer, which is
/// unlikely if you want zero-copy functionality.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
pub fn read_fixext8<R: Read>(rd: &mut R) -> Result<(i8, [u8; 8]), ValueReadError> {
    match read_marker(rd)? {
        Marker::FixExt8 => {
            let mut buf = [0; 8];
            read_fixext_data(rd, &mut buf).map(|ty| (ty, buf))
        }
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

/// Attempts to read exactly 18 bytes from the given reader and interpret them as a fixext16 type
/// with data attached.
///
/// According to the MessagePack specification, a fixext16 stores an integer and a byte array whose
/// length is 16 bytes. Its marker byte is `0xd8`.
///
/// Note, that this function copies a byte array from the reader to the output buffer, which is
/// unlikely if you want zero-copy functionality.
///
/// # Errors
///
/// This function will return `ValueReadError` on any I/O error while reading either the marker or
/// the data.
pub fn read_fixext16<R: Read>(rd: &mut R) -> Result<(i8, [u8; 16]), ValueReadError> {
    match read_marker(rd)? {
        Marker::FixExt16 => {
            let mut buf = [0; 16];
            read_fixext_data(rd, &mut buf).map(|ty| (ty, buf))
        }
        marker => Err(ValueReadError::TypeMismatch(marker)),
    }
}

fn read_fixext_data<R: Read>(rd: &mut R, buf: &mut [u8]) -> Result<i8, ValueReadError> {
    let id = read_data_i8(rd)?;
    match rd.read_exact(buf) {
        Ok(()) => Ok(id),
        Err(err) => Err(ValueReadError::InvalidDataRead(err)),
    }
}

/// Extension type meta information.
///
/// Extension represents a tuple of type information and a byte array where type information is an
/// integer whose meaning is defined by applications.
///
/// Applications can assign 0 to 127 to store application-specific type information.
///
/// # Note
///
/// MessagePack reserves -1 to -128 for future extension to add predefined types which will be
/// described in separated documents.
#[derive(Debug, PartialEq)]
pub struct ExtMeta {
    /// Type information.
    pub typeid: i8,
    /// Byte array size.
    pub size: u32,
}

pub fn read_ext_meta<R: Read>(rd: &mut R) -> Result<ExtMeta, ValueReadError> {
    let size = match read_marker(rd)? {
        Marker::FixExt1 => 1,
        Marker::FixExt2 => 2,
        Marker::FixExt4 => 4,
        Marker::FixExt8 => 8,
        Marker::FixExt16 => 16,
        Marker::Ext8 => read_data_u8(rd)? as u32,
        Marker::Ext16 => read_data_u16(rd)? as u32,
        Marker::Ext32 => read_data_u32(rd)?,
        marker => return Err(ValueReadError::TypeMismatch(marker)),
    };

    let ty = read_data_i8(rd)?;
    let meta = ExtMeta {
        typeid: ty,
        size,
    };

    Ok(meta)
}
