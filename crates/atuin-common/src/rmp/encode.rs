//! MessagePack encode helpers built on [`rmp::encode`].

use rmp::encode as mp;
use rmp::encode::{RmpWrite, RmpWriteErr, ValueWriteError};

// Re-export rmp's write primitives so decoders/encoders depend only on this module.
pub use rmp::encode::{
    write_array_len, write_bin, write_bool, write_sint, write_str, write_u8, write_u16, write_u64,
    write_uint,
};

/// An error encountered while encoding a MessagePack value.
#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

/// Write an optional value, encoding [`None`] as a nil marker.
pub fn write_optional<W: RmpWrite, T>(
    out: &mut W,
    value: Option<T>,
    write: impl FnOnce(&mut W, T) -> Result<(), ValueWriteError<W::Error>>,
) -> Result<(), EncodeError<W::Error>> {
    match value {
        Some(v) => write(out, v).map_err(EncodeError::from),
        None => mp::write_nil(out)
            .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e))),
    }
}
