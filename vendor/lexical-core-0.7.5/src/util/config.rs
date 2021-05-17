//! Config settings for lexical-core.

use crate::lib::slice;
use super::algorithm::copy_to_dst;
use super::rounding::RoundingKind;

// HELPERS

/// Determine if the character is a control character for integers or floats.
/// Control characters include digits, `.`, `+`, and `-`.
fn is_control_character(ch: u8, is_default: bool) -> bool {
    if is_default {
        // Default character handles radixes < 15 (where 'e'/'E' is a
        // a valid exponent character).
        match ch as char {
            '0' ..= '9'     => true,
            'a' ..= 'd'     => true,
            'A' ..= 'D'     => true,
            '.' | '+' | '-' => true,
            _               => false,
        }
    } else {
        // Backup character handles radixes >= 15 (where no digit is
        // a valid exponent character).
        match ch as char {
            '0' ..= '9'     => true,
            'a' ..= 'z'     => true,
            'A' ..= 'Z'     => true,
            '.' | '+' | '-' => true,
            _               => false,
        }
    }
}

// Check if byte array starts with case-insensitive N.
#[inline]
fn starts_with_n(bytes: &[u8]) -> bool {
    match bytes.get(0) {
        Some(&b'N') => true,
        Some(&b'n') => true,
        _           => false,
    }
}

// Check if byte array starts with case-insensitive I.
#[inline]
fn starts_with_i(bytes: &[u8]) -> bool {
    match bytes.get(0) {
        Some(&b'I') => true,
        Some(&b'i') => true,
        _           => false,
    }
}

/// Fixed-size string for float configurations.
///
/// These values are guaranteed less than or equal to the maximum
/// number of bytes after a sign byte has been written.
pub(crate) struct FloatConfigString {
    /// Storage data for the config string.
    data: [u8; F32_FORMATTED_SIZE - 1],
    /// Actual length of the data.
    length: usize,
}

impl FloatConfigString {
    /// Reset data from a byte string.
    pub(crate) fn load_bytes(&mut self, bytes: &[u8]) {
        assert!(bytes.len() <= self.data.len());
        copy_to_dst(&mut self.data, bytes);
        self.length = bytes.len();
    }

    /// Convert to byte slice.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // Always safe, since length can only be set from `load_bytes`.
        unsafe {
            self.data.get_unchecked(..self.length)
        }
    }
}

// GLOBALS

/// Default character for scientific notation, used when the `radix < 15`.
static mut EXPONENT_DEFAULT_CHAR: u8 = b'e';

/// Backup character for scientific notation, used when the `radix >= 15`.
#[cfg(feature ="radix")]
static mut EXPONENT_BACKUP_CHAR: u8 = b'^';

/// The rounding scheme for float conversions.
#[cfg(feature = "rounding")]
static mut FLOAT_ROUNDING: RoundingKind = RoundingKind::NearestTieEven;

cfg_if! {
if #[cfg(feature = "radix")] {
    /// Not a Number literal.
    static mut NAN_STRING: FloatConfigString = FloatConfigString {
        // b"NaN"
        data: [b'N', b'a', b'N', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 3
    };

    /// Short infinity literal.
    static mut INF_STRING: FloatConfigString = FloatConfigString {
        // b"inf"
        data: [b'i', b'n', b'f', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 3
    };

    /// Long infinity literal.
    static mut INFINITY_STRING: FloatConfigString = FloatConfigString {
        // b"infinity"
        data: [b'i', b'n', b'f', b'i', b'n', b'i', b't', b'y', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 8
    };
} else {
    /// Not a Number literal.
    static mut NAN_STRING: FloatConfigString = FloatConfigString {
        // b"NaN"
        data: [b'N', b'a', b'N', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 3
    };

    /// Short infinity literal.
    static mut INF_STRING: FloatConfigString = FloatConfigString {
        // b"inf"
        data: [b'i', b'n', b'f', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 3
    };

    /// Long infinity literal.
    static mut INFINITY_STRING: FloatConfigString = FloatConfigString {
        // b"infinity"
        data: [b'i', b'n', b'f', b'i', b'n', b'i', b't', b'y', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0'],
        length: 8
    };
}}   // cfg_if

// GETTERS/SETTERS

/// Get default character for the exponent symbol.
///
/// Default character for scientific notation, used when the `radix < 15`.
#[inline]
pub fn get_exponent_default_char() -> u8
{
    unsafe {
        EXPONENT_DEFAULT_CHAR
    }
}

/// Set the default character for the exponent symbol.
///
/// Default character for scientific notation, used when the `radix < 15`.
///
/// To change the expected, default character for an exponent,
/// change this value before using lexical.
///
/// * `ch`      - Character for exponent symbol.
///
/// # Safety
///
/// Do not call this function in threaded-code, as it is not thread-safe.
/// Do not call this function after compiling any formats.
///
/// # Panics
///
/// Panics if the character is in the character set `[A-Da-d.+\-]`.
#[inline]
pub unsafe fn set_exponent_default_char(ch: u8)
{
    assert!(!is_control_character(ch, true));
    EXPONENT_DEFAULT_CHAR = ch
}

/// Get backup character for the exponent symbol.
///
/// For numerical strings of `radix >= 15`, 'e' or 'E' is a valid digit,
/// and therefore may no longer be used as a marker for the exponent.
#[inline]
#[cfg(feature ="radix")]
pub fn get_exponent_backup_char() -> u8
{
    unsafe {
        EXPONENT_BACKUP_CHAR
    }
}

/// Set the backup character for the exponent symbol.
///
/// For numerical strings of `radix >= 15`, 'e' or 'E' is a valid digit,
/// and therefore may no longer be used as a marker for the exponent.
///
/// To change the expected, backup character for an exponent,
/// change this value before using lexical.
///
/// * `ch`      - Character for exponent symbol.
///
/// # Safety
///
/// Do not call this function in threaded-code, as it is not thread-safe.
/// Do not call this function after compiling any formats.
///
/// # Panics
///
/// Panics if the character is in the character set `[A-Za-z.+\-]`.
#[inline]
#[cfg(feature ="radix")]
pub unsafe fn set_exponent_backup_char(ch: u8)
{
    assert!(!is_control_character(ch, false));
    EXPONENT_BACKUP_CHAR = ch
}

/// Get the default rounding scheme for float conversions.
///
/// This defines the global rounding-scheme for float parsing operations.
/// By default, this is set to `RoundingKind::NearestTieEven`. IEEE754
/// recommends this as the default for all for decimal and binary
/// operations.
#[inline]
#[cfg(feature = "rounding")]
pub fn get_float_rounding() -> RoundingKind {
    unsafe {
        FLOAT_ROUNDING
    }
}

/// Set the default rounding scheme for float conversions.
///
/// This defines the global rounding-scheme for float parsing operations.
/// By default, this is set to `RoundingKind::NearestTieEven`. IEEE754
/// recommends this as the default for all for decimal and binary
/// operations.
///
/// # Safety
///
/// Do not modify this value in threaded-code, as it is not thread-safe.
#[inline]
#[cfg(feature = "rounding")]
pub unsafe fn set_float_rounding(rounding: RoundingKind) {
    FLOAT_ROUNDING = rounding
}

/// Get string representation of Not a Number as a byte slice.
#[inline]
pub fn get_nan_string() -> &'static [u8]
{
    unsafe {
        NAN_STRING.as_bytes()
    }
}

/// Set representation of Not a Number from a byte slice.
///
/// * `bytes`    - Slice of bytes to assign as NaN string representation.
///
/// # Safety
///
/// Do not call this function in threaded-code, as it is not thread-safe.
///
/// # Panics
///
/// Panics if:
/// - `bytes.len() >= f32::FORMATTED_SIZE`
/// - `bytes` is empty
/// - `bytes` does not start with an `'N'` or `'n'`.
#[inline]
pub unsafe fn set_nan_string(bytes: &[u8])
{
    assert!(starts_with_n(bytes));
    NAN_STRING.load_bytes(bytes);
}

/// Get the short representation of an Infinity literal as a byte slice.
#[inline]
pub fn get_inf_string() -> &'static [u8]
{
    unsafe {
        INF_STRING.as_bytes()
    }
}

/// Set the short representation of Infinity from a byte slice.
///
/// * `bytes`    - Slice of bytes to assign as Infinity string representation.
///
/// # Safety
///
/// Do not call this function in threaded-code, as it is not thread-safe.
///
/// # Panics
///
/// Panics if:
/// - `bytes.len() >= f32::FORMATTED_SIZE`
/// - `bytes.len() >= get_infinity_string().len()`
/// - `bytes` is empty
/// - `bytes` does not start with an `'I'` or `'i'`.
#[inline]
pub unsafe fn set_inf_string(bytes: &[u8])
{
    assert!(starts_with_i(bytes) && bytes.len() <= INFINITY_STRING.length);
    INF_STRING.load_bytes(bytes);
}

/// Get the long representation of an Infinity literal as a byte slice.
#[inline]
pub fn get_infinity_string() -> &'static [u8]
{
    unsafe {
        INFINITY_STRING.as_bytes()
    }
}

/// Set the long representation of Infinity from a byte slice.
///
/// * `bytes`    - Slice of bytes to assign as Infinity string representation.
///
/// # Safety
///
/// Do not call this function in threaded-code, as it is not thread-safe.
///
/// # Panics
///
/// Panics if:
/// - `bytes.len() >= f32::FORMATTED_SIZE`
/// - `bytes.len() < get_inf_string().len()`
/// - `bytes` is empty
/// - `bytes` does not start with an `'I'` or `'i'`.
#[inline]
pub unsafe fn set_infinity_string(bytes: &[u8])
{
    assert!(starts_with_i(bytes) && bytes.len() >= INF_STRING.length);
    INFINITY_STRING.load_bytes(bytes);
}

// CONSTANTS

// The f64 buffer is actually a size of 60, but use 64 since it's a
// power of 2.
pub(crate) const I8_FORMATTED_SIZE_DECIMAL: usize = 4;
pub(crate) const I16_FORMATTED_SIZE_DECIMAL: usize = 6;
pub(crate) const I32_FORMATTED_SIZE_DECIMAL: usize = 11;
pub(crate) const I64_FORMATTED_SIZE_DECIMAL: usize = 20;
pub(crate) const U8_FORMATTED_SIZE_DECIMAL: usize = 3;
pub(crate) const U16_FORMATTED_SIZE_DECIMAL: usize = 5;
pub(crate) const U32_FORMATTED_SIZE_DECIMAL: usize = 10;
pub(crate) const U64_FORMATTED_SIZE_DECIMAL: usize = 20;
pub(crate) const F32_FORMATTED_SIZE_DECIMAL: usize = 64;
pub(crate) const F64_FORMATTED_SIZE_DECIMAL: usize = 64;
pub(crate) const I128_FORMATTED_SIZE_DECIMAL: usize = 40;
pub(crate) const U128_FORMATTED_SIZE_DECIMAL: usize = 39;

// Simple, fast optimization.
// Since we're declaring a variable on the stack, and our power-of-two
// alignment dramatically improved atoi performance, do it.
cfg_if! {
if #[cfg(feature = "radix")] {
    // Use 256, actually, since we seem to have memory issues with f64.
    // Clearly not sufficient memory allocated for non-decimal values.
    pub(crate) const I8_FORMATTED_SIZE: usize = 16;
    pub(crate) const I16_FORMATTED_SIZE: usize = 32;
    pub(crate) const I32_FORMATTED_SIZE: usize = 64;
    pub(crate) const I64_FORMATTED_SIZE: usize = 128;
    pub(crate) const U8_FORMATTED_SIZE: usize = 16;
    pub(crate) const U16_FORMATTED_SIZE: usize = 32;
    pub(crate) const U32_FORMATTED_SIZE: usize = 64;
    pub(crate) const U64_FORMATTED_SIZE: usize = 128;
    pub(crate) const F32_FORMATTED_SIZE: usize = 256;
    pub(crate) const F64_FORMATTED_SIZE: usize = 256;
    pub(crate) const I128_FORMATTED_SIZE: usize = 256;
    pub(crate) const U128_FORMATTED_SIZE: usize = 256;
} else {
    // The f64 buffer is actually a size of 60, but use 64 since it's a
    // power of 2.
    pub(crate) const I8_FORMATTED_SIZE: usize = I8_FORMATTED_SIZE_DECIMAL;
    pub(crate) const I16_FORMATTED_SIZE: usize = I16_FORMATTED_SIZE_DECIMAL;
    pub(crate) const I32_FORMATTED_SIZE: usize = I32_FORMATTED_SIZE_DECIMAL;
    pub(crate) const I64_FORMATTED_SIZE: usize = I64_FORMATTED_SIZE_DECIMAL;
    pub(crate) const U8_FORMATTED_SIZE: usize = U8_FORMATTED_SIZE_DECIMAL;
    pub(crate) const U16_FORMATTED_SIZE: usize = U16_FORMATTED_SIZE_DECIMAL;
    pub(crate) const U32_FORMATTED_SIZE: usize = U32_FORMATTED_SIZE_DECIMAL;
    pub(crate) const U64_FORMATTED_SIZE: usize = U64_FORMATTED_SIZE_DECIMAL;
    pub(crate) const F32_FORMATTED_SIZE: usize = F32_FORMATTED_SIZE_DECIMAL;
    pub(crate) const F64_FORMATTED_SIZE: usize = F64_FORMATTED_SIZE_DECIMAL;
    pub(crate) const I128_FORMATTED_SIZE: usize = I128_FORMATTED_SIZE_DECIMAL;
    pub(crate) const U128_FORMATTED_SIZE: usize = U128_FORMATTED_SIZE_DECIMAL;
}} // cfg_if

cfg_if! {
if #[cfg(target_pointer_width = "16")] {
    pub(crate) const ISIZE_FORMATTED_SIZE: usize = I16_FORMATTED_SIZE;
    pub(crate) const ISIZE_FORMATTED_SIZE_DECIMAL: usize = I16_FORMATTED_SIZE_DECIMAL;
    pub(crate) const USIZE_FORMATTED_SIZE: usize = U16_FORMATTED_SIZE;
    pub(crate) const USIZE_FORMATTED_SIZE_DECIMAL: usize = U16_FORMATTED_SIZE_DECIMAL;
} else if #[cfg(target_pointer_width = "32")] {
    pub(crate) const ISIZE_FORMATTED_SIZE: usize = I32_FORMATTED_SIZE;
    pub(crate) const ISIZE_FORMATTED_SIZE_DECIMAL: usize = I32_FORMATTED_SIZE_DECIMAL;
    pub(crate) const USIZE_FORMATTED_SIZE: usize = U32_FORMATTED_SIZE;
    pub(crate) const USIZE_FORMATTED_SIZE_DECIMAL: usize = U32_FORMATTED_SIZE_DECIMAL;
} else if #[cfg(target_pointer_width = "64")] {
    pub(crate) const ISIZE_FORMATTED_SIZE: usize = I64_FORMATTED_SIZE;
    pub(crate) const ISIZE_FORMATTED_SIZE_DECIMAL: usize = I64_FORMATTED_SIZE_DECIMAL;
    pub(crate) const USIZE_FORMATTED_SIZE: usize = U64_FORMATTED_SIZE;
    pub(crate) const USIZE_FORMATTED_SIZE_DECIMAL: usize = U64_FORMATTED_SIZE_DECIMAL;
}}  // cfg_if

/// Maximum number of bytes required to serialize any number to string.
pub const BUFFER_SIZE: usize = F64_FORMATTED_SIZE;

// FUNCTIONS

/// Get the exponent notation character.
#[inline]
#[allow(unused_variables)]
pub(crate) fn exponent_notation_char(radix: u32) -> u8 {
    #[cfg(not(feature ="radix"))] {
        get_exponent_default_char()
    }

    #[cfg(feature ="radix")] {
        if radix >= 15 {
            get_exponent_backup_char()
        } else {
            get_exponent_default_char()
        }
    }
}

// TEST
// ----

#[cfg(test)]
mod tests {
    use crate::util::*;
    use crate::util::test::*;
    use super::*;

    #[cfg(feature ="radix")]
    #[test]
    fn exponent_notation_char_test() {
        let default = get_exponent_default_char();
        let backup = get_exponent_backup_char();
        assert_eq!(exponent_notation_char(2), default);
        assert_eq!(exponent_notation_char(8), default);
        assert_eq!(exponent_notation_char(10), default);
        assert_eq!(exponent_notation_char(15), backup);
        assert_eq!(exponent_notation_char(16), backup);
        assert_eq!(exponent_notation_char(32), backup);
    }

    // Only enable when no other threads touch NAN_STRING or INFINITY_STRING.
    #[test]
    #[ignore]
    fn special_bytes_test() {
        unsafe {
            let mut buffer = new_buffer();
            // Test serializing and deserializing special strings.
            assert!(f32::from_lexical(b"NaN").unwrap().is_nan());
            assert!(f32::from_lexical(b"nan").unwrap().is_nan());
            assert!(f32::from_lexical(b"NAN").unwrap().is_nan());
            assert!(f32::from_lexical(b"inf").unwrap().is_infinite());
            assert!(f32::from_lexical(b"INF").unwrap().is_infinite());
            assert!(f32::from_lexical(b"Infinity").unwrap().is_infinite());
            assert_eq!(f64::NAN.to_lexical(&mut buffer), b"NaN");
            assert_eq!(f64::INFINITY.to_lexical(&mut buffer), b"inf");

            set_nan_string(b"nan");
            set_inf_string(b"Infinity");

            assert!(f32::from_lexical(b"inf").err().unwrap().code == ErrorCode::InvalidDigit);
            assert!(f32::from_lexical(b"Infinity").unwrap().is_infinite());
            assert_eq!(f64::NAN.to_lexical(&mut buffer), b"nan");
            assert_eq!(f64::INFINITY.to_lexical(&mut buffer), b"Infinity");

            set_nan_string(b"NaN");
            set_inf_string(b"inf");
        }
    }

    // Only enable when no other threads touch FLOAT_ROUNDING.
    #[cfg(all(feature = "correct", feature = "rounding"))]
    #[test]
    #[ignore]
    fn special_rounding_test() {
        // Each one of these pairs is halfway, and we can detect the
        // rounding schemes from this.
        unsafe {
            // Nearest, tie-even
            set_float_rounding(RoundingKind::NearestTieEven);
            assert_eq!(f64::from_lexical(b"-9007199254740993").unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical(b"-9007199254740995").unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical(b"9007199254740993").unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical(b"9007199254740995").unwrap(), 9007199254740996.0);

            // Nearest, tie-away-zero
            set_float_rounding(RoundingKind::NearestTieAwayZero);
            assert_eq!(f64::from_lexical(b"-9007199254740993").unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical(b"-9007199254740995").unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical(b"9007199254740993").unwrap(), 9007199254740994.0);
            assert_eq!(f64::from_lexical(b"9007199254740995").unwrap(), 9007199254740996.0);

            // Toward positive infinity
            set_float_rounding(RoundingKind::TowardPositiveInfinity);
            assert_eq!(f64::from_lexical(b"-9007199254740993").unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical(b"-9007199254740995").unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical(b"9007199254740993").unwrap(), 9007199254740994.0);
            assert_eq!(f64::from_lexical(b"9007199254740995").unwrap(), 9007199254740996.0);

            // Toward negative infinity
            set_float_rounding(RoundingKind::TowardNegativeInfinity);
            assert_eq!(f64::from_lexical(b"-9007199254740993").unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical(b"-9007199254740995").unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical(b"9007199254740993").unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical(b"9007199254740995").unwrap(), 9007199254740994.0);

            // Toward zero
            set_float_rounding(RoundingKind::TowardZero);
            assert_eq!(f64::from_lexical(b"-9007199254740993").unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical(b"-9007199254740995").unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical(b"9007199254740993").unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical(b"9007199254740995").unwrap(), 9007199254740994.0);

            // Reset to default
            set_float_rounding(RoundingKind::NearestTieEven);
        }
    }

    // Only enable when no other threads touch FLOAT_ROUNDING.
    #[cfg(all(feature = "correct", feature = "radix", feature = "rounding"))]
    #[test]
    #[ignore]
    fn special_rounding_binary_test() {
        // Each one of these pairs is halfway, and we can detect the
        // rounding schemes from this.
        unsafe {
            // Nearest, tie-even
            set_float_rounding(RoundingKind::NearestTieEven);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000001", 2).unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000011", 2).unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000001", 2).unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000011", 2).unwrap(), 9007199254740996.0);

            // Nearest, tie-away-zero
            set_float_rounding(RoundingKind::NearestTieAwayZero);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000001", 2).unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000011", 2).unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000001", 2).unwrap(), 9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000011", 2).unwrap(), 9007199254740996.0);

            // Toward positive infinity
            set_float_rounding(RoundingKind::TowardPositiveInfinity);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000001", 2).unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000011", 2).unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000001", 2).unwrap(), 9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000011", 2).unwrap(), 9007199254740996.0);

            // Toward negative infinity
            set_float_rounding(RoundingKind::TowardNegativeInfinity);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000001", 2).unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000011", 2).unwrap(), -9007199254740996.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000001", 2).unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000011", 2).unwrap(), 9007199254740994.0);

            // Toward zero
            set_float_rounding(RoundingKind::TowardZero);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000001", 2).unwrap(), -9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"-100000000000000000000000000000000000000000000000000011", 2).unwrap(), -9007199254740994.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000001", 2).unwrap(), 9007199254740992.0);
            assert_eq!(f64::from_lexical_radix(b"100000000000000000000000000000000000000000000000000011", 2).unwrap(), 9007199254740994.0);

            // Reset to default
            set_float_rounding(RoundingKind::NearestTieEven);
        }
    }

    #[should_panic]
    #[test]
    fn set_exponent_default_char_digit_test() {
        unsafe {
            set_exponent_default_char(b'0')
        }
    }

    #[should_panic]
    #[test]
    fn set_exponent_default_char_period_test() {
        unsafe {
            set_exponent_default_char(b'.')
        }
    }

    #[should_panic]
    #[test]
    fn set_exponent_default_char_add_test() {
        unsafe {
            set_exponent_default_char(b'+')
        }
    }

    #[should_panic]
    #[test]
    fn set_exponent_default_char_sub_test() {
        unsafe {
            set_exponent_default_char(b'-')
        }
    }

    #[cfg(all(feature = "radix"))]
    #[should_panic]
    #[test]
    fn set_exponent_backup_char_digit_test() {
        unsafe {
            set_exponent_backup_char(b'0')
        }
    }

    #[cfg(all(feature = "radix"))]
    #[should_panic]
    #[test]
    fn set_exponent_backup_char_period_test() {
        unsafe {
            set_exponent_backup_char(b'.')
        }
    }

    #[cfg(all(feature = "radix"))]
    #[should_panic]
    #[test]
    fn set_exponent_backup_char_add_test() {
        unsafe {
            set_exponent_backup_char(b'+')
        }
    }

    #[cfg(all(feature = "radix"))]
    #[should_panic]
    #[test]
    fn set_exponent_backup_char_sub_test() {
        unsafe {
            set_exponent_backup_char(b'-')
        }
    }

    #[should_panic]
    #[test]
    fn set_nan_string_empty_test() {
        unsafe {
            set_nan_string(b"")
        }
    }

    #[should_panic]
    #[test]
    fn set_nan_string_invalid_test() {
        unsafe {
            set_nan_string(b"i")
        }
    }

    #[should_panic]
    #[test]
    fn set_inf_string_empty_test() {
        unsafe {
            set_inf_string(b"")
        }
    }

    #[should_panic]
    #[test]
    fn set_inf_string_invalid_test() {
        unsafe {
            set_inf_string(b"n")
        }
    }

    #[should_panic]
    #[test]
    fn set_inf_string_long_test() {
        unsafe {
            set_inf_string(b"infinityinfinf")
        }
    }

    #[should_panic]
    #[test]
    fn set_infinity_string_empty_test() {
        unsafe {
            set_infinity_string(b"")
        }
    }

    #[should_panic]
    #[test]
    fn set_infinity_string_invalid_test() {
        unsafe {
            set_infinity_string(b"n")
        }
    }

    #[should_panic]
    #[test]
    fn set_infinity_string_short_test() {
        unsafe {
            set_infinity_string(b"i")
        }
    }
}
