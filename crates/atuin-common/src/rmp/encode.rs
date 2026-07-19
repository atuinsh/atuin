//! MessagePack encode helpers built on [`rmp::encode`].

pub use rmp::encode::*;

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
        None => {
            write_nil(out).map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e)))
        }
    }
}
