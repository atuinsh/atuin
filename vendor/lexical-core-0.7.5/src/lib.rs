//! Fast lexical conversion routines for a no_std environment.
//!
//! lexical-core is a low-level API for number-to-string and
//! string-to-number conversions, without requiring a system
//! allocator. If you would like to use a convenient, high-level
//! API, please look at [lexical](https://crates.io/crates/lexical)
//! instead.
//!
//! # Getting Started
//!
//! ```rust
//! extern crate lexical_core;
//!
//! // String to number using Rust slices.
//! // The argument is the byte string parsed.
//! let f: f32 = lexical_core::parse(b"3.5").unwrap();   // 3.5
//! let i: i32 = lexical_core::parse(b"15").unwrap();    // 15
//!
//! // All lexical_core parsers are checked, they validate the
//! // input data is entirely correct, and stop parsing when invalid data
//! // is found, or upon numerical overflow.
//! let r = lexical_core::parse::<u8>(b"256"); // Err(ErrorCode::Overflow.into())
//! let r = lexical_core::parse::<u8>(b"1a5"); // Err(ErrorCode::InvalidDigit.into())
//!
//! // In order to extract and parse a number from a substring of the input
//! // data, use `parse_partial`. These functions return the parsed value and
//! // the number of processed digits, allowing you to extract and parse the
//! // number in a single pass.
//! let r = lexical_core::parse_partial::<i8>(b"3a5"); // Ok((3, 1))
//!
//! // If an insufficiently long buffer is passed, the serializer will panic.
//! // PANICS
//! let mut buf = [b'0'; 1];
//! //let slc = lexical_core::write::<i64>(15, &mut buf);
//!
//! // In order to guarantee the buffer is long enough, always ensure there
//! // are at least `T::FORMATTED_SIZE` bytes, which requires the
//! // `lexical_core::Number` trait to be in scope.
//! use lexical_core::Number;
//! let mut buf = [b'0'; f64::FORMATTED_SIZE];
//! let slc = lexical_core::write::<f64>(15.1, &mut buf);
//! assert_eq!(slc, b"15.1");
//!
//! // When the `radix` feature is enabled, for decimal floats, using
//! // `T::FORMATTED_SIZE` may significantly overestimate the space
//! // required to format the number. Therefore, the
//! // `T::FORMATTED_SIZE_DECIMAL` constants allow you to get a much
//! // tighter bound on the space required.
//! let mut buf = [b'0'; f64::FORMATTED_SIZE_DECIMAL];
//! let slc = lexical_core::write::<f64>(15.1, &mut buf);
//! assert_eq!(slc, b"15.1");
//! ```
//!
//! # Conversion API
//!
//! **To String**
//! - [`write`]
#![cfg_attr(feature = "radix", doc = " - [`write_radix`]")]
//!
//! **From String**
//! - [`parse`]
#![cfg_attr(feature = "radix", doc = " - [`parse_radix`]")]
#![cfg_attr(feature = "format", doc = " - [`parse_format`]")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " - [`parse_format_radix`]")]
//! - [`parse_partial`]
#![cfg_attr(feature = "radix", doc = " - [`parse_partial_radix`]")]
#![cfg_attr(feature = "format", doc = " - [`parse_partial_format`]")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " - [`parse_partial_format_radix`]")]
//! - [`parse_lossy`]
#![cfg_attr(feature = "radix", doc = " - [`parse_lossy_radix`]")]
#![cfg_attr(feature = "format", doc = " - [`parse_lossy_format`]")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " - [`parse_lossy_format_radix`]")]
//! - [`parse_partial_lossy`]
#![cfg_attr(feature = "radix", doc = " - [`parse_partial_lossy_radix`]")]
#![cfg_attr(feature = "format", doc = " - [`parse_partial_lossy_format`]")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " - [`parse_partial_lossy_format_radix`]")]
//!
//! # Configuration Settings
//!
//! **Get Configuration**
//! - [`get_exponent_default_char`]
#![cfg_attr(feature = "radix", doc = " - [`get_exponent_backup_char`]")]
#![cfg_attr(all(feature = "correct", feature = "rounding"), doc = " - [`get_float_rounding`]")]
//! - [`get_nan_string`]
//! - [`get_inf_string`]
//! - [`get_infinity_string`]
//!
//! **Set Configuration**
//! - [`set_exponent_default_char`]
#![cfg_attr(feature = "radix", doc = " - [`set_exponent_backup_char`]")]
#![cfg_attr(all(feature = "correct", feature = "rounding"), doc = " - [`set_float_rounding`]")]
//! - [`set_nan_string`]
//! - [`set_inf_string`]
//! - [`set_infinity_string`]
//!
//! [`write`]: fn.write.html
#![cfg_attr(feature = "radix", doc = " [`write_radix`]: fn.write_radix.html")]
//! [`parse`]: fn.parse.html
#![cfg_attr(feature = "radix", doc = " [`parse_radix`]: fn.parse_radix.html")]
#![cfg_attr(feature = "format", doc = " [`parse_format`]: fn.parse_format.html")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " [`parse_format_radix`]: fn.parse_format_radix.html")]
//! [`parse_partial`]: fn.parse_partial.html
#![cfg_attr(feature = "radix", doc = " [`parse_partial_radix`]: fn.parse_partial_radix.html")]
#![cfg_attr(feature = "format", doc = " [`parse_partial_format`]: fn.parse_partial_format.html")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " [`parse_partial_format_radix`]: fn.parse_partial_format_radix.html")]
//! [`parse_lossy`]: fn.parse_lossy.html
#![cfg_attr(feature = "radix", doc = " [`parse_lossy_radix`]: fn.parse_lossy_radix.html")]
#![cfg_attr(feature = "format", doc = " [`parse_lossy_format`]: fn.parse_lossy_format.html")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " [`parse_lossy_format_radix`]: fn.parse_lossy_format_radix.html")]
//! [`parse_partial_lossy`]: fn.parse_partial_lossy.html
#![cfg_attr(feature = "radix", doc = " [`parse_partial_lossy_radix`]: fn.parse_partial_lossy_radix.html")]
#![cfg_attr(feature = "format", doc = " [`parse_partial_lossy_format`]: fn.parse_partial_lossy_format.html")]
#![cfg_attr(all(feature = "format", feature = "radix"), doc = " [`parse_partial_lossy_format_radix`]: fn.parse_partial_lossy_format_radix.html")]
//!
//! [`get_exponent_default_char`]: fn.get_exponent_default_char.html
#![cfg_attr(feature = "radix", doc = " [`get_exponent_backup_char`]: fn.get_exponent_backup_char.html")]
#![cfg_attr(all(feature = "correct", feature = "rounding"), doc = " [`get_float_rounding`]: fn.get_float_rounding.html")]
//! [`get_nan_string`]: fn.get_nan_string.html
//! [`get_inf_string`]: fn.get_inf_string.html
//! [`get_infinity_string`]: fn.get_infinity_string.html
//!
//! [`set_exponent_default_char`]: fn.set_exponent_default_char.html
#![cfg_attr(feature = "radix", doc = " [`set_exponent_backup_char`]: fn.set_exponent_backup_char.html")]
#![cfg_attr(all(feature = "correct", feature = "rounding"), doc = " [`set_float_rounding`]: fn.set_float_rounding.html")]
//! [`set_nan_string`]: fn.set_nan_string.html
//! [`set_inf_string`]: fn.set_inf_string.html
//! [`set_infinity_string`]: fn.set_infinity_string.html

// FEATURES

// Require intrinsics in a no_std context.
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(feature = "std"), feature(core_intrinsics))]

// DEPENDENCIES

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate cfg_if;

#[cfg(any(feature = "correct", feature = "format"))]
#[macro_use]
extern crate static_assertions;

// Testing assertions for floating-point equality.
#[cfg(test)]
#[macro_use]
extern crate approx;

// Test against randomly-generated data.
#[cfg(test)]
#[macro_use]
extern crate quickcheck;

// Test against randomly-generated guided data.
#[cfg(all(test, feature = "std"))]
#[macro_use]
extern crate proptest;

// Use vec if there is a system allocator, which we require only if
// we're using the correct and radix features.
#[cfg(all(not(feature = "std"), feature = "correct", feature = "radix"))]
#[cfg_attr(test, macro_use)]
extern crate alloc;

// Use arrayvec for atof.
#[cfg(feature = "correct")]
extern crate arrayvec;

// Ensure only one back-end is enabled.
#[cfg(all(feature = "grisu3", feature = "ryu"))]
compile_error!("Lexical only accepts one of the following backends: `grisu3` or `ryu`.");

// Import the back-end, if applicable.
cfg_if! {
if #[cfg(feature = "grisu3")] {
    extern crate dtoa;
} else if #[cfg(feature = "ryu")] {
    extern crate ryu;
}}  // cfg_if

/// Facade around the core features for name mangling.
pub(crate) mod lib {
#[cfg(feature = "std")]
pub(crate) use std::*;

#[cfg(not(feature = "std"))]
pub(crate) use core::*;

cfg_if! {
if #[cfg(all(feature = "correct", feature = "radix"))] {
    #[cfg(feature = "std")]
    pub(crate) use std::vec::Vec;

    #[cfg(not(feature = "std"))]
    pub(crate) use ::alloc::vec::Vec;
}}  // cfg_if

}   // lib

// API

// Hide implementation details
#[macro_use]
mod util;

mod atof;
mod atoi;
mod float;
mod ftoa;
mod itoa;

// Re-export configuration and utilities globally.
pub use util::*;

/// Write number to string.
///
/// Returns a subslice of the input buffer containing the written bytes,
/// starting from the same address in memory as the input slice.
///
/// * `value`   - Number to serialize.
/// * `bytes`   - Slice containing a numeric string.
///
/// # Panics
///
/// Panics if the buffer may not be large enough to hold the serialized
/// number. In order to ensure the function will not panic, provide a
/// buffer with at least [`FORMATTED_SIZE_DECIMAL`] elements.
///
/// [`FORMATTED_SIZE_DECIMAL`]: trait.Number.html#associatedconstant.FORMATTED_SIZE_DECIMAL
#[inline]
pub fn write<'a, N: ToLexical>(n: N, bytes: &'a mut [u8])
    -> &'a mut [u8]
{
    n.to_lexical(bytes)
}

/// Write number to string with a custom radix.
///
/// Returns a subslice of the input buffer containing the written bytes,
/// starting from the same address in memory as the input slice.
///
/// * `value`   - Number to serialize.
/// * `radix`   - Radix for number encoding.
/// * `bytes`   - Slice containing a numeric string.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
///
/// Also panics if the buffer may not be large enough to hold the
/// serialized number. In order to ensure the function will not panic,
/// provide a buffer with at least [`FORMATTED_SIZE`] elements.
///
/// [`FORMATTED_SIZE`]: trait.Number.html#associatedconstant.FORMATTED_SIZE
#[inline]
#[cfg(feature = "radix")]
pub fn write_radix<'a, N: ToLexical>(n: N, radix: u8, bytes: &'a mut [u8])
    -> &'a mut [u8]
{
    n.to_lexical_radix(radix, bytes)
}

/// Parse number from string.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing.
///
/// * `bytes`   - Byte slice containing a numeric string.
#[inline]
pub fn parse<N: FromLexical>(bytes: &[u8])
    -> Result<N>
{
    N::from_lexical(bytes)
}

/// Parse number from string.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point.
///
/// * `bytes`   - Byte slice containing a numeric string.
#[inline]
pub fn parse_partial<N: FromLexical>(bytes: &[u8])
    -> Result<(N, usize)>
{
    N::from_lexical_partial(bytes)
}

/// Lossily parse number from string.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. This parser is
/// lossy, so numerical rounding may occur during parsing.
///
/// * `bytes`   - Byte slice containing a numeric string.
#[inline]
pub fn parse_lossy<N: FromLexicalLossy>(bytes: &[u8])
    -> Result<N>
{
    N::from_lexical_lossy(bytes)
}

/// Lossily parse number from string.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. This parser is
/// lossy, so numerical rounding may occur during parsing.
///
/// * `bytes`   - Byte slice containing a numeric string.
#[inline]
pub fn parse_partial_lossy<N: FromLexicalLossy>(bytes: &[u8])
    -> Result<(N, usize)>
{
    N::from_lexical_partial_lossy(bytes)
}

/// Parse number from string with a custom radix.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing.
///
/// * `radix`   - Radix for number decoding.
/// * `bytes`   - Byte slice containing a numeric string.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(feature = "radix")]
pub fn parse_radix<N: FromLexical>(bytes: &[u8], radix: u8)
    -> Result<N>
{
    N::from_lexical_radix(bytes, radix)
}

/// Parse number from string with a custom radix.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point.
///
/// * `radix`   - Radix for number decoding.
/// * `bytes`   - Byte slice containing a numeric string.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(feature = "radix")]
pub fn parse_partial_radix<N: FromLexical>(bytes: &[u8], radix: u8)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_radix(bytes, radix)
}

/// Lossily parse number from string with a custom radix.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. This parser is
/// lossy, so numerical rounding may occur during parsing.
///
/// * `radix`   - Radix for number decoding.
/// * `bytes`   - Byte slice containing a numeric string.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(feature = "radix")]
pub fn parse_lossy_radix<N: FromLexicalLossy>(bytes: &[u8], radix: u8)
    -> Result<N>
{
    N::from_lexical_lossy_radix(bytes, radix)
}

/// Lossily parse number from string with a custom radix.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. This parser is
/// lossy, so numerical rounding may occur during parsing.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `radix`   - Radix for number decoding.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(feature = "radix")]
pub fn parse_partial_lossy_radix<N: FromLexicalLossy>(bytes: &[u8], radix: u8)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_lossy_radix(bytes, radix)
}

/// Parse number from string with a custom numerical format.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. The numerical format
/// is specified by the format bitflags, which customize the required
/// components, digit separators, and other parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `format`  - Numerical format.
#[inline]
#[cfg(feature = "format")]
pub fn parse_format<N: FromLexicalFormat>(bytes: &[u8], format: NumberFormat)
    -> Result<N>
{
    N::from_lexical_format(bytes, format)
}

/// Parse number from string with a custom numerical format.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. The numerical format
/// is specified by the format bitflags, which customize the required
/// components, digit separators, and other parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `format`  - Numerical format.
#[inline]
#[cfg(feature = "format")]
pub fn parse_partial_format<N: FromLexicalFormat>(bytes: &[u8], format: NumberFormat)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_format(bytes, format)
}

/// Lossily parse number from string with a custom numerical format.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. This parser is
/// lossy, so numerical rounding may occur during parsing. The
/// numerical format is specified by the format bitflags, which
/// customize the required components, digit separators, and other
/// parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `format`  - Numerical format.
#[inline]
#[cfg(feature = "format")]
pub fn parse_lossy_format<N: FromLexicalLossyFormat>(bytes: &[u8], format: NumberFormat)
    -> Result<N>
{
    N::from_lexical_lossy_format(bytes, format)
}

/// Lossily parse number from string with a custom numerical format.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. This parser is
/// lossy, so numerical rounding may occur during parsing. The
/// numerical format is specified by the format bitflags, which
/// customize the required components, digit separators, and other
/// parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `format`  - Numerical format.
#[inline]
#[cfg(feature = "format")]
pub fn parse_partial_lossy_format<N: FromLexicalLossyFormat>(bytes: &[u8], format: NumberFormat)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_lossy_format(bytes, format)
}

/// Parse number from string with a custom radix and numerical format.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. The numerical format
/// is specified by the format bitflags, which customize the required
/// components, digit separators, and other parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `radix`   - Radix for number decoding.
/// * `format`  - Numerical format.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(all(feature = "radix", feature = "format"))]
pub fn parse_format_radix<N: FromLexicalFormat>(bytes: &[u8], radix: u8, format: NumberFormat)
    -> Result<N>
{
    N::from_lexical_format_radix(bytes, radix, format)
}

/// Parse number from string with a custom radix and numerical format.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. The numerical format
/// is specified by the format bitflags, which customize the required
/// components, digit separators, and other parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `radix`   - Radix for number decoding.
/// * `format`  - Numerical format.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(all(feature = "radix", feature = "format"))]
pub fn parse_partial_format_radix<N: FromLexicalFormat>(bytes: &[u8], radix: u8, format: NumberFormat)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_format_radix(bytes, radix, format)
}

/// Lossily parse number from string with a custom radix and numerical format.
///
/// This method parses the entire string, returning an error if
/// any invalid digits are found during parsing. This parser is
/// lossy, so numerical rounding may occur during parsing. The
/// numerical format is specified by the format bitflags, which
/// customize the required components, digit separators, and other
/// parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `radix`   - Radix for number decoding.
/// * `format`  - Numerical format.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(all(feature = "radix", feature = "format"))]
pub fn parse_lossy_format_radix<N: FromLexicalLossyFormat>(bytes: &[u8], radix: u8, format: NumberFormat)
    -> Result<N>
{
    N::from_lexical_lossy_format_radix(bytes, radix, format)
}

/// Lossily parse number from string with a custom radix and numerical format.
///
/// This method parses until an invalid digit is found (or the end
/// of the string), returning the number of processed digits
/// and the parsed value until that point. This parser is
/// lossy, so numerical rounding may occur during parsing. The
/// numerical format is specified by the format bitflags, which
/// customize the required components, digit separators, and other
/// parameters of the number.
///
/// * `bytes`   - Byte slice containing a numeric string.
/// * `radix`   - Radix for number decoding.
/// * `format`  - Numerical format.
///
/// # Panics
///
/// Panics if the radix is not in the range `[2, 36]`.
#[inline]
#[cfg(all(feature = "radix", feature = "format"))]
pub fn parse_partial_lossy_format_radix<N: FromLexicalLossyFormat>(bytes: &[u8], radix: u8, format: NumberFormat)
    -> Result<(N, usize)>
{
    N::from_lexical_partial_lossy_format_radix(bytes, radix, format)
}
