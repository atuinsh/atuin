//! String-to-integer routines specialized to parse the mantissa of a float.

use crate::util::*;

#[cfg(feature = "correct")]
use super::shared::*;

// STANDALONE MANTISSA
// -------------------

// These routines are a specialized parser for the mantissa of a floating-
// point number, from two buffers containing valid digits. We want to
// exit early on numeric overflow, returning the value parsed up until
// that point.

// Calculate the mantissa and the number of truncated digits from a digits iterator.
// Will stop once the iterators produce a non-valid digit character.
perftools_inline!{
#[cfg(feature = "correct")]
pub(crate) fn standalone_mantissa<'a, T, Iter1, Iter2>(mut integer: Iter1, mut fraction: Iter2, radix: u32)
    -> (T, usize)
    where T: UnsignedInteger,
          Iter1: Iterator<Item=&'a u8>,
          Iter2: Iterator<Item=&'a u8>
{
    // Mote:
    //  Do not use iter.chain(), since it is enormously slow.
    //  Since we need to maintain backwards compatibility, even if
    //  iter.chain() is patched, for older Rustc versions, it's nor
    //  worth the performance penalty.

    let mut value: T = T::ZERO;
    // On overflow, validate that all the remaining characters are valid
    // digits, if not, return the first invalid digit. Otherwise,
    // calculate the number of truncated digits.
    while let Some(c) = integer.next() {
        value = match add_digit(value, to_digit!(*c, radix).unwrap(), radix) {
            Some(v) => v,
            None    => {
                let truncated = 1 + integer.count() + fraction.count();
                return (value, truncated);
            },
        };
    }
    while let Some(c) = fraction.next() {
        value = match add_digit(value, to_digit!(*c, radix).unwrap(), radix) {
            Some(v) => v,
            None    => {
                let truncated = 1 + fraction.count();
                return (value, truncated);
            },
        };
    }
    (value, 0)
}}

// Calculate the mantissa when it cannot have sign or other invalid digits.
perftools_inline!{
#[cfg(not(feature = "correct"))]
pub(crate) fn standalone_mantissa<'a, T, Iter>(mut iter: Iter, radix: u32)
    -> T
    where T: Integer,
          Iter: Iterator<Item=&'a u8>
{
    // Parse the integer.
    let mut value = T::ZERO;
    while let Some(c) = iter.next() {
        // Cannot overflow, since using a wrapped float.
        value = (value * as_cast(radix)) + as_cast(to_digit!(*c, radix).unwrap());
    }
   value
}}

// Calculate mantissa and only take first N digits.
perftools_inline!{
#[cfg(not(feature = "correct"))]
pub(crate) fn standalone_mantissa_n<'a, T, Iter>(iter: &mut Iter, radix: u32, max: usize)
    -> (T, usize)
    where T: Integer,
          Iter: Iterator<Item=&'a u8>
{
    // Parse the integer.
    let mut value = T::ZERO;
    let mut index = 0;
    while index < max {
        if let Some(c) = iter.next() {
            // Cannot overflow, since we're limiting it to non-overflowing digit counts.
            index += 1;
            value = (value * as_cast(radix)) + as_cast(to_digit!(*c, radix).unwrap());
        } else {
            break;
        }
    }
    (value, index)
}}
