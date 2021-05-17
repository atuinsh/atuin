//! parsers recognizing numbers

#[macro_use]
mod macros;

pub mod streaming;
pub mod complete;

/// Configurable endianness
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Endianness {
  /// big endian
  Big,
  /// little endian
  Little,
}

