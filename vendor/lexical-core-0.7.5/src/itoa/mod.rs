//! Integer-to-string formatting routines.

// Hide internal implementation details.
#[cfg(feature = "table")]
mod decimal;

#[cfg(all(feature = "table", feature = "radix"))]
mod generic;

#[cfg(not(feature = "table"))]
mod naive;

mod api;

#[cfg(feature = "radix")]
pub(crate) use self::api::itoa_positive;
