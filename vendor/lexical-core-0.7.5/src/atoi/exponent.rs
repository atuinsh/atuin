//! String-to-integer routines specialized to parse the exponent of a float.

use crate::util::*;
use super::shared::*;

// STANDALONE EXPONENT
// -------------------

// These routines are a specialized parser for the exponent of a floating-
// point number, from an unvalidated buffer with a potential sign bit.
// On numeric overflow or underflow, we want to return the max or min
// value possible, respectively. On overflow, find the first non-digit
// char (if applicable), and return the max/min value and the number
// of digits parsed.

// Add digit to mantissa.
macro_rules! add_digit {
    ($value:ident, $radix:ident, $op:ident, $digit:ident) => {
        match $value.checked_mul(as_cast($radix)) {
            Some(v) => v.$op(as_cast($digit)),
            None    => None,
        }
    };
}

// Iterate over the digits and iteratively process them.
macro_rules! parse_digits_exponent {
    ($value:ident, $iter:ident, $radix:ident, $op:ident, $default:expr) => (
        while let Some(c) = $iter.next() {
            let digit = match to_digit(c, $radix) {
                Ok(v)  => v,
                Err(c) => return ($value, c),
            };
            $value = match add_digit!($value, $radix, $op, digit) {
                Some(v) => v,
                None    => {
                    // Consume the rest of the iterator to validate
                    // the remaining data.
                    if let Some(c) = $iter.find(|&c| is_not_digit_char(*c, $radix)) {
                        return ($default, c);
                    }
                    $default
                },
            };
        }
    );
}

// Specialized parser for the exponent, which validates digits and
// returns a default min or max value on overflow.
perftools_inline!{
pub(crate) fn standalone_exponent<'a, Iter>(mut iter: Iter, radix: u32, sign: Sign)
    -> (i32, *const u8)
    where Iter: AsPtrIterator<'a, u8>
{
    // Parse the sign bit or current data.
    let mut value = 0;
    match sign {
        Sign::Positive => parse_digits_exponent!(value, iter, radix, checked_add, i32::max_value()),
        Sign::Negative => parse_digits_exponent!(value, iter, radix, checked_sub, i32::min_value())
    }

    (value, iter.as_ptr())
}}
