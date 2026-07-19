//! MessagePack encode helpers built on [`rmp::encode`].
//!
//! Re-exports `rmp::encode`'s `write_*` primitives so encoders depend only on
//! this module, wraps their error in [`EncodeError`] (which converts cleanly
//! into [`eyre::Report`]), and adds a nil-aware [`write_optional`].

use rmp::encode as mp;
use rmp::encode::{RmpWrite, RmpWriteErr, ValueWriteError};

// Re-export rmp's write primitives so decoders/encoders depend only on this module.
pub use rmp::encode::{
    write_array_len, write_bin, write_bool, write_sint, write_str, write_u8, write_u16, write_u64,
    write_uint,
};

/// An error encountered while encoding a MessagePack value.
///
/// Wraps [`ValueWriteError`] with a message that names the failing write and its
/// inner I/O error — neither of which `rmp`'s own [`Display`] reports. Implements
/// [`std::error::Error`], so it converts into [`eyre::Report`] with `?`.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

/// Write an optional value, encoding [`None`] as a nil marker. `write` encodes
/// the inner value when present (e.g. [`write_u64`] or [`write_str`]).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rmp::decode::{self, Bytes};
    use pretty_assertions::assert_eq;

    #[test]
    fn write_optional_some_then_read_back() {
        let mut out = Vec::new();
        write_optional(&mut out, Some(99u64), write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(
            decode::read_optional(&mut b, decode::read_u64).unwrap(),
            Some(99)
        );
    }

    #[test]
    fn write_optional_none_writes_nil() {
        let mut out = Vec::new();
        write_optional::<_, u64>(&mut out, None, write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(
            decode::read_optional(&mut b, decode::read_u64).unwrap(),
            None
        );
    }

    #[test]
    fn write_optional_str() {
        let mut out = Vec::new();
        write_optional(&mut out, Some("hi"), write_str).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(
            decode::read_optional(&mut b, decode::read_string).unwrap(),
            Some("hi".to_string())
        );
    }
}
