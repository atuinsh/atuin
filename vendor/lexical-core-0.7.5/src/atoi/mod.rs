//! Fast lexical string-to-integer conversion routines.

// Hide implementation details.
#[macro_use]
mod shared;

mod api;
mod exponent;
mod generic;
mod mantissa;

// Re-exports
pub(crate) use self::mantissa::*;
pub(crate) use self::exponent::*;
