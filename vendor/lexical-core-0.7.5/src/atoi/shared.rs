//! Shared definitions for string-to-integer conversions.

use crate::lib::result::Result as StdResult;

#[cfg(feature = "correct")]
use crate::util::*;

// SHARED
// ------

// Convert u8 to digit.
macro_rules! to_digit {
    ($c:expr, $radix:expr) => (($c as char).to_digit($radix));
}

// Parse the sign bit and filter empty inputs from the atoi data.
macro_rules! parse_sign {
    ($bytes:ident, $is_signed:expr, $code:ident) => ({
        // Filter out empty inputs.
        if $bytes.is_empty() {
            return Err((ErrorCode::$code, $bytes.as_ptr()));
        }

        let (sign, digits) = match index!($bytes[0]) {
            b'+'               => (Sign::Positive, &index!($bytes[1..])),
            b'-' if $is_signed => (Sign::Negative, &index!($bytes[1..])),
            _                  => (Sign::Positive, $bytes),
        };

        // Filter out empty inputs.
        if digits.is_empty() {
            return Err((ErrorCode::$code, digits.as_ptr()));
        }

        (sign, digits)
    });
}

// Get pointer to 1-past-end of slice.
// Performance note: Use slice, as `iter.as_ptr()` turns out to
// quite slow performance wise, likely since it needs to calculate
// the end ptr, while for a slice this is effectively a no-op.
perftools_inline_always!{
pub(super) fn last_ptr<T>(slc: &[T]) -> *const T {
    index!(slc[slc.len()..]).as_ptr()
}}

// Convert character to digit.
perftools_inline_always!{
pub(super) fn to_digit<'a>(c: &'a u8, radix: u32) -> StdResult<u32, &'a u8> {
    match to_digit!(*c, radix) {
        Some(v) => Ok(v),
        None    => Err(c),
    }
}}

// Convert character to digit.
perftools_inline_always!{
pub(super) fn is_not_digit_char(c: u8, radix: u32) -> bool {
    to_digit!(c, radix).is_none()
}}

// Add digit to mantissa.
perftools_inline_always!{
#[cfg(feature = "correct")]
pub(super) fn add_digit<T>(value: T, digit: u32, radix: u32)
    -> Option<T>
    where T: UnsignedInteger
{
    return value
        .checked_mul(as_cast(radix))?
        .checked_add(as_cast(digit))
}}
