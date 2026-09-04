//! Byte counts and the limits expressed in terms of them, in the text forms used by
//! `config.toml` (`1MB`, `10%`, `unlimited`).

mod byte_size;
mod text_or_bytes;

pub use byte_size::{ByteSize, ByteSizeParseError};
