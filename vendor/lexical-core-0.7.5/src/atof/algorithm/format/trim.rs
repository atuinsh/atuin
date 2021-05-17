//! Trim leading and trailing 0s and digit separators.

use crate::util::*;

// TRIM

// Trim leading 0s.
// Does not consume any digit separators.
perftools_inline!{
pub(super) fn ltrim_zero_no_separator<'a>(bytes: &'a [u8], _: u8)
    -> (&'a [u8], usize)
{
    ltrim_char_slice(bytes, b'0')
}}

// Trim leading 0s and digit separators.
perftools_inline!{
#[cfg(feature = "format")]
pub(super) fn ltrim_zero_separator<'a>(bytes: &'a [u8], digit_separator: u8)
    -> (&'a [u8], usize)
{
    ltrim_char2_slice(bytes, b'0', digit_separator)
}}

// Trim leading digit separators (so, nothing).
// Does not consume any digit separators.
perftools_inline!{
pub(super) fn ltrim_separator_no_separator<'a>(bytes: &'a [u8], _: u8)
    -> (&'a [u8], usize)
{
    (bytes, 0)
}}

// Trim leading digit separators.
perftools_inline!{
#[cfg(feature = "format")]
pub(super) fn ltrim_separator_separator<'a>(bytes: &'a [u8], digit_separator: u8)
    -> (&'a [u8], usize)
{
    ltrim_char_slice(bytes, digit_separator)
}}

// Trim trailing 0s.
// Does not consume any digit separators.
perftools_inline!{
pub(super) fn rtrim_zero_no_separator<'a>(bytes: &'a [u8], _: u8)
    -> (&'a [u8], usize)
{
    rtrim_char_slice(bytes, b'0')
}}

// Trim trailing 0s and digit separators.
perftools_inline!{
#[cfg(feature = "format")]
pub(super) fn rtrim_zero_separator<'a>(bytes: &'a [u8], digit_separator: u8)
    -> (&'a [u8], usize)
{
    rtrim_char2_slice(bytes, b'0', digit_separator)
}}

// Trim trailing digit separators (so, nothing).
// Does not consume any digit separators.
perftools_inline!{
pub(super) fn rtrim_separator_no_separator<'a>(bytes: &'a [u8], _: u8)
    -> (&'a [u8], usize)
{
    (bytes, 0)
}}

// Trim trailing digit separators.
perftools_inline!{
#[cfg(feature = "format")]
pub(super) fn rtrim_separator_separator<'a>(bytes: &'a [u8], digit_separator: u8)
    -> (&'a [u8], usize)
{
    rtrim_char_slice(bytes, digit_separator)
}}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_zero_no_separator_test() {
        assert_eq!(ltrim_zero_no_separator(b!("01"), b'_'), (b!("1"), 1));
        assert_eq!(rtrim_zero_no_separator(b!("23450"), b'_'), (b!("2345"), 1));
    }

    #[test]
    fn trim_separator_no_separator_test() {
        assert_eq!(ltrim_separator_no_separator(b!("01"), b'_'), (b!("01"), 0));
        assert_eq!(rtrim_separator_no_separator(b!("23450"), b'_'), (b!("23450"), 0));
    }

    #[test]
    #[cfg(feature = "format")]
    fn trim_zero_iltc_separator_test() {
        assert_eq!(ltrim_zero_separator(b!("0_1_2"), b'_'), (b!("1_2"), 2));
        assert_eq!(rtrim_zero_separator(b!("2345_0_"), b'_'), (b!("2345"), 3));
    }

    #[test]
    #[cfg(feature = "format")]
    fn trim_separator_iltc_separator_test() {
        assert_eq!(ltrim_separator_separator(b!("0_1_2"), b'_'), (b!("0_1_2"), 0));
        assert_eq!(rtrim_separator_separator(b!("2345_0_"), b'_'), (b!("2345_0"), 1));
    }
}
