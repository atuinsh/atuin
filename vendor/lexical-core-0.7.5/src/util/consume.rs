//! Algorithms to consume digits and digit separators.
//!
//! # Complexity
//!
//! Although superficially quite simple, the level of complexity
//! introduced by digit separators can be quite complex, due
//! the number of permutations during parsing.
//!
//! We can consume any combinations of of [0,3] items from the following set:
//!     - [l]eading digit separators, where digit separators occur before digits.
//!     - [i]nternal digit separators, where digit separators occur between digits.
//!     - [t]railing digit separators, where digit separators occur after digits.
//!
//! In addition to those combinations, we can also have:
//!     - [c]onsecutive digit separators, which allows two digit separators to be adjacent.
//!
//! # Shorthand
//!
//! We will use the term consumer to denote a function that consumes digits,
//! splitting an input buffer at an index, where the leading section contains
//! valid input digits, and the trailing section contains invalid characters.
//! Due to the number of combinations for consumers, we use the following
//! shorthand to denote consumers:
//!     - `no`, does not use a digit separator.
//!     - `l`, consumes leading digit separators.
//!     - `i`, consumes internal digit separators.
//!     - `t`, consumes trailing digit separators.
//!     - `c`, consumes consecutive digit separators.
//!
//! Consumers are named `consume_digits_x_separator`, where `x` represents
//! the shorthand name of the consumer, in sorted order. For example,
//! `consume_digits_ilt` means that consumer can consume
//! internal, leading, and trailing digit separators, but not
//! consecutive ones.
//!
//! # Signature
//!
//! All low-level consumers have the following signature:
//!
//! ```text
//! fn consumer<'a>(
//!     digits: &'a [u8],
//!     radix: u32,
//!     digit_separator: u8
//! ) -> (&'a [u8], &'a [u8]);
//! ```
//!
//! All high-level consumers have the following signature:
//!
//! ```text
//! fn consumer<'a>(
//!     digits: &'a [u8],
//!     radix: u32,
//!     format: NumberFormat
//! ) -> (&'a [u8], &'a [u8]);
//! ```
//!
//! If the consumer does not require a digit separator, that value is
//! simply ignored.

use super::format::*;

// HELPERS

// Convert radix to value.
macro_rules! to_digit {
    ($c:expr, $radix:expr) => (($c as char).to_digit($radix));
}

// Convert character to digit.
#[inline(always)]
fn is_digit(c: u8, radix: u32) -> bool {
    to_digit!(c, radix).is_some()
}

// Convert character to digit.
#[cfg(feature = "format")]
#[inline(always)]
fn is_digit_or_separator(c: u8, radix: u32, digit_separator: u8) -> bool {
    return is_digit(c, radix) || c == digit_separator
}

// Split buffer at index.
#[inline(always)]
fn split_at_index<'a>(digits: &'a [u8], index: usize)
    -> (&'a [u8], &'a [u8])
{
    (&digits[..index], &digits[index..])
}

// CONSUMERS

// We use the following convention to denote consumers:
//  consume_digits_x, where `x` can be:
//      - , does not use a digit separator.
//      - l, consumes leading digit separators.
//      - i, consumes internal digit separators.
//      - t, consumes trailing digit separators.
//      - c, consumes consecutive digit separators.
//
// It then can use any permutation of [lit], with an optional [c] for
// each permutation, or use `` for no permutation.

// Consume until a an invalid digit is found.
// Does not consume any digit separators.
#[inline]
fn consume_digits<'a>(digits: &'a [u8], radix: u32, _: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all digits.
    let mut index = 0;
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }
    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes internal digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_i<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all digits and internal digit separators, except for
    // consecutive digit separators.
    let mut previous = false;
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
            previous = false;
        } else if c == digit_separator && index != 0 && !previous {
            index += 1;
            previous = true;
        } else {
            break;
        }
    }

    // We've gone too far if:
    //      1). The last character was a digit separator.
    if previous {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes internal and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_ic<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all characters that are digits or digit separators, except
    // for a leading digit separator.
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
        } else if c == digit_separator && index != 0 {
            index += 1;
        } else {
            break;
        }
    }

    // We've gone too far if:
    //      1). The trailing digits are digit separators.
    // Preconditions:
    //      1). If index > 0, we know digits[0] has to a digit.
    while index > 1 && index!(digits[index-1]) == digit_separator {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_l<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume leading digit separator, if applicable.
    let mut index = 0;
    if index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // Consume all interior digits.
    // Store the previous index to later determine if any digits
    // were consumed.
    let prev_index = index;
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // We've gone too far if:
    //      1). We consumed no interior digits.
    //      2). The next character is a digit separator (cannot be a digit).
    if prev_index == index && index < digits.len() && index!(digits[index]) == digit_separator {
        index = 0;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_lc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all leading digit separators, if applicable.
    let mut index = 0;
    while index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // Consume all interior digits.
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // We cannot have gone too far, because in order to be in an invalid
    // state, we would have to consume 0 digits and the next character
    // be a digit separator, which is impossible since we greedily
    // consume leading digit separators.

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes trailing digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_t<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all interior digits.
    let mut index = 0;
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // Consume a trailing digit separator, if applicable.
    // Store the previous index to later determine if a digit separator
    // was consumed.
    let prev_index = index;
    if index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // We have gone too far if:
    //      1). We consumed a trailing digit separator.
    //      2). The next character is a digit or digit separator.
    if index != prev_index && index < digits.len() && is_digit_or_separator(index!(digits[index]), radix, digit_separator) {
        index = prev_index;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes trailing and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_tc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all interior digits.
    let mut index = 0;
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // Consume all trailing digit separators, if applicable.
    // Store the previous index to later determine if any digit
    // separators were consumed.
    let prev_index = index;
    while index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // We have gone too far if:
    //      1). We consumed more than 1 trailing digit separators.
    //      2). The next character is a digit (cannot be a digit separator).
    if index != prev_index && index < digits.len() && is_digit(index!(digits[index]), radix) {
        index = prev_index;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading and internal digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_il<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume digits and digit separators, until consecutive digit
    // separators or invalid characters.
    let mut previous = false;
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
            previous = false;
        } else if c == digit_separator && !previous {
            index += 1;
            previous = true;
        } else {
            break;
        }
    }

    // We've taken everything except consecutive digit separators.
    // We've gone too far if:
    //      1). The last index was a digit separator unless:
    //          1). The current index is 1 (index 0 was a digit separator).
    //          2). The current character is not a digit separator (cannot be a digit).
    if previous && !(index == 1 && index < digits.len() && index!(digits[index]) != digit_separator) {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading and internal digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_ilc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume digits and digit separators until an invalid character.
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit_or_separator(c, radix, digit_separator) {
            index += 1;
        } else {
            break;
        }
    }

    // We've taken everything except invalid characters.
    // We have gone too far if:
    //      1). We have trailing digit separators.
    // Remove all trailing digit separators, however, store the index in
    // case all are removed.
    let current_index = index;
    while index >= 1 && index!(digits[index-1]) == digit_separator {
        index -= 1;
    }

    // All trailing digit separators were removed (or current_index is 0).
    // Reset back to current index.
    if index == 0 {
        index = current_index;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes internal and trailing digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_it<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all characters that are digits or digit separators, except
    // leading and consecutive digit separators.
    let mut previous = false;
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
            previous = false;
        } else if c == digit_separator && index != 0 && !previous {
            index += 1;
            previous = true;
        } else {
            break;
        }
    }

    // We needed the check for `index != 0` to ensure we don't consume
    // buffers like b"_123_". However, We might not have gotten a
    // trailing separator if:
    //      1). The index was 0, something like b"_.".
    if index == 0 && index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
        previous = true;
    }

    // We've taken up to 1 leading digit separator, or anything
    // except consecutive digit separators. We've gone too far if:
    //      1). We take consecutive digit separators.
    //      2). The next character is a digit (only occurs from special index == 9 check).
    if previous && index < digits.len() && is_digit_or_separator(index!(digits[index]), radix, digit_separator) {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes internal, trailing, and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_itc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all characters that are digits or digit separators, except
    // for a leading digit separator.
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
        } else if c == digit_separator && index != 0 {
            index += 1;
        } else {
            break;
        }
    }

    // We needed to check for `index != 0` to ensure we don't consume
    // buffers like b"_123_". However, We might not have gotten a
    // trailing separator if:
    //      1). The index was 0, something like b"_." or b"__.".
    if index == 0 {
        // Consume all leading digit separators.
        while index < digits.len() && index!(digits[index]) == digit_separator {
            index += 1;
        }

        // Now, we might have gone too far. If the next character is a digit,
        // we need to rollback to 0.
        if index < digits.len() && is_digit(index!(digits[index]), radix) {
            index = 0;
        }
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading and trailing digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_lt<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume leading digit separator, if applicable.
    let mut index = 0;
    if index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // Consume all interior digits.
    // Store the previous index to later determine if any digits
    // were consumed.
    let prev_index = index;
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // Consume a trailing digit separator. If we haven't consumed any digits,
    // then we have a leading b'__', so we shouldn't consume that either.
    let mut previous = index == prev_index;
    if !previous && index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
        previous = true;
    }

    // We have gone too far if:
    //      1). The last character was a digit separator.
    //      2). The current character is a digit or digit separator.
    if index < digits.len() && previous && is_digit_or_separator(index!(digits[index]), radix, digit_separator) {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading, trailing, and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_ltc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume all leading digit separators, if applicable.
    let mut index = 0;
    while index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // Consume all interior digits.
    // We don't need to store the index, because if we consume no digits,
    // then the next character cannot possibly be a digit separator.
    while index < digits.len() && is_digit(index!(digits[index]), radix) {
        index += 1;
    }

    // Consume all trailing digit separators.
    let prev_index = index;
    while index < digits.len() && index!(digits[index]) == digit_separator {
        index += 1;
    }

    // We have gone too far if:
    //      1). We consumed trailing digit separators.
    //      2). The subsequent character is a digit (cannot be a digit separator).
    if index < digits.len() && index != prev_index && is_digit(index!(digits[index]), radix) {
        index = prev_index;
    }

    split_at_index(digits, index)
}


// Consume until a an invalid digit is found.
// Consumes leading, internal, and trailing digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_ilt<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume digits and digit separators, until consecutive digit
    // separators or invalid characters.
    let mut previous = false;
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit(c, radix) {
            index += 1;
            previous = false;
        } else if c == digit_separator && !previous {
            index += 1;
            previous = true;
        } else {
            break;
        }
    }

    // We've taken everything except consecutive digit separators.
    // That means we've gone too far if:
    //      1). The last character was a digit separator.
    //      2). The current character is a digit separator.
    if previous && index < digits.len() && index!(digits[index]) == digit_separator {
        index -= 1;
    }

    split_at_index(digits, index)
}

// Consume until a an invalid digit is found.
// Consumes leading, internal, trailing, and consecutive digit separators.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_iltc<'a>(digits: &'a [u8], radix: u32, digit_separator: u8)
    -> (&'a [u8], &'a [u8])
{
    // Consume digits and digit separators, until an invalid character.
    // There is no post-condition since we accept any digit or
    // digit separator combination.
    let mut index = 0;
    while index < digits.len() {
        let c = index!(digits[index]);
        if is_digit_or_separator(c, radix, digit_separator) {
            index += 1;
        } else {
            break;
        }
    }

    split_at_index(digits, index)
}

// API

// Consume digits without a digit separator.
#[inline]
pub(crate) fn consume_digits_no_separator<'a>(bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> (&'a [u8], &'a [u8])
{
    consume_digits(bytes, radix, format.digit_separator())
}

// Consume digits while ignoring the digit separator.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_digits_ignore_separator<'a>(bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> (&'a [u8], &'a [u8])
{
    consume_digits_iltc(bytes, radix, format.digit_separator())
}

// Consume digits with a digit separator in the integer component.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_integer_digits_separator<'a>(bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> (&'a [u8], &'a [u8])
{
    const I: NumberFormat = NumberFormat::INTEGER_INTERNAL_DIGIT_SEPARATOR;
    const L: NumberFormat = NumberFormat::INTEGER_LEADING_DIGIT_SEPARATOR;
    const T: NumberFormat = NumberFormat::INTEGER_TRAILING_DIGIT_SEPARATOR;
    const C: NumberFormat = NumberFormat::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR;
    const IL: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | L.bits());
    const IT: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | T.bits());
    const LT: NumberFormat = NumberFormat::from_bits_truncate(L.bits() | T.bits());
    const ILT: NumberFormat = NumberFormat::from_bits_truncate(IL.bits() | T.bits());
    const IC: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | C.bits());
    const LC: NumberFormat = NumberFormat::from_bits_truncate(L.bits() | C.bits());
    const TC: NumberFormat = NumberFormat::from_bits_truncate(T.bits() | C.bits());
    const ILC: NumberFormat = NumberFormat::from_bits_truncate(IL.bits() | C.bits());
    const ITC: NumberFormat = NumberFormat::from_bits_truncate(IT.bits() | C.bits());
    const LTC: NumberFormat = NumberFormat::from_bits_truncate(LT.bits() | C.bits());
    const ILTC: NumberFormat = NumberFormat::from_bits_truncate(ILT.bits() | C.bits());

    let digit_separator = format.digit_separator();
    match format & NumberFormat::INTEGER_DIGIT_SEPARATOR_FLAG_MASK {
        I       => consume_digits_i(bytes, radix, digit_separator),
        IC      => consume_digits_ic(bytes, radix, digit_separator),
        L       => consume_digits_l(bytes, radix, digit_separator),
        LC      => consume_digits_lc(bytes, radix, digit_separator),
        T       => consume_digits_t(bytes, radix, digit_separator),
        TC      => consume_digits_tc(bytes, radix, digit_separator),
        IL      => consume_digits_il(bytes, radix, digit_separator),
        ILC     => consume_digits_ilc(bytes, radix, digit_separator),
        IT      => consume_digits_it(bytes, radix, digit_separator),
        ITC     => consume_digits_itc(bytes, radix, digit_separator),
        LT      => consume_digits_lt(bytes, radix, digit_separator),
        LTC     => consume_digits_ltc(bytes, radix, digit_separator),
        ILT     => consume_digits_ilt(bytes, radix, digit_separator),
        ILTC    => consume_digits_iltc(bytes, radix, digit_separator),
        _       => unreachable!()
    }
}

// Consume digits with a digit separator in the fraction component.
#[inline]
#[cfg(feature = "format")]
pub(crate) fn consume_fraction_digits_separator<'a>(bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> (&'a [u8], &'a [u8])
{
    const I: NumberFormat = NumberFormat::FRACTION_INTERNAL_DIGIT_SEPARATOR;
    const L: NumberFormat = NumberFormat::FRACTION_LEADING_DIGIT_SEPARATOR;
    const T: NumberFormat = NumberFormat::FRACTION_TRAILING_DIGIT_SEPARATOR;
    const C: NumberFormat = NumberFormat::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR;
    const IL: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | L.bits());
    const IT: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | T.bits());
    const LT: NumberFormat = NumberFormat::from_bits_truncate(L.bits() | T.bits());
    const ILT: NumberFormat = NumberFormat::from_bits_truncate(IL.bits() | T.bits());
    const IC: NumberFormat = NumberFormat::from_bits_truncate(I.bits() | C.bits());
    const LC: NumberFormat = NumberFormat::from_bits_truncate(L.bits() | C.bits());
    const TC: NumberFormat = NumberFormat::from_bits_truncate(T.bits() | C.bits());
    const ILC: NumberFormat = NumberFormat::from_bits_truncate(IL.bits() | C.bits());
    const ITC: NumberFormat = NumberFormat::from_bits_truncate(IT.bits() | C.bits());
    const LTC: NumberFormat = NumberFormat::from_bits_truncate(LT.bits() | C.bits());
    const ILTC: NumberFormat = NumberFormat::from_bits_truncate(ILT.bits() | C.bits());

    let digit_separator = format.digit_separator();
    match format & NumberFormat::FRACTION_DIGIT_SEPARATOR_FLAG_MASK {
        I       => consume_digits_i(bytes, radix, digit_separator),
        IC      => consume_digits_ic(bytes, radix, digit_separator),
        L       => consume_digits_l(bytes, radix, digit_separator),
        LC      => consume_digits_lc(bytes, radix, digit_separator),
        T       => consume_digits_t(bytes, radix, digit_separator),
        TC      => consume_digits_tc(bytes, radix, digit_separator),
        IL      => consume_digits_il(bytes, radix, digit_separator),
        ILC     => consume_digits_ilc(bytes, radix, digit_separator),
        IT      => consume_digits_it(bytes, radix, digit_separator),
        ITC     => consume_digits_itc(bytes, radix, digit_separator),
        LT      => consume_digits_lt(bytes, radix, digit_separator),
        LTC     => consume_digits_ltc(bytes, radix, digit_separator),
        ILT     => consume_digits_ilt(bytes, radix, digit_separator),
        ILTC    => consume_digits_iltc(bytes, radix, digit_separator),
        _       => unreachable!()
    }
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consume_digits_test() {
        assert_eq!(consume_digits(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits(b!("_.45"), 10, b'_'), (b!(""), b!("_.45")));
        assert_eq!(consume_digits(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits(b!("_.45_5"), 10, b'_'), (b!(""), b!("_.45_5")));
        assert_eq!(consume_digits(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));
    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_l_test() {
        assert_eq!(consume_digits_l(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_l(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_l(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_l(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_l(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_l(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_l(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_l(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_l(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_l(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_l(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_l(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_l(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_l(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_l(b!("_45_5"), 10, b'_'), (b!("_45"), b!("_5")));
        assert_eq!(consume_digits_l(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_l(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_l(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_l(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_l(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_l(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_l(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_l(b!("_45_"), 10, b'_'), (b!("_45"), b!("_")));
        assert_eq!(consume_digits_l(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_l(b!("_45_.56"), 10, b'_'), (b!("_45"), b!("_.56")));
        assert_eq!(consume_digits_l(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_l(b!("_4_5_"), 10, b'_'), (b!("_4"), b!("_5_")));
        assert_eq!(consume_digits_l(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_l(b!("_4_5_.56"), 10, b'_'), (b!("_4"), b!("_5_.56")));
        assert_eq!(consume_digits_l(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_lc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_lc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_lc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_lc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_lc(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_lc(b!("__45"), 10, b'_'), (b!("__45"), b!("")));
        assert_eq!(consume_digits_lc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_lc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_lc(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_lc(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_lc(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_lc(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_lc(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_lc(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_lc(b!("_45_5"), 10, b'_'), (b!("_45"), b!("_5")));
        assert_eq!(consume_digits_lc(b!("__45__5"), 10, b'_'), (b!("__45"), b!("__5")));
        assert_eq!(consume_digits_lc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_lc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_lc(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_lc(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_lc(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_lc(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_lc(b!("_45_"), 10, b'_'), (b!("_45"), b!("_")));
        assert_eq!(consume_digits_lc(b!("__45__"), 10, b'_'), (b!("__45"), b!("__")));
        assert_eq!(consume_digits_lc(b!("_45_.56"), 10, b'_'), (b!("_45"), b!("_.56")));
        assert_eq!(consume_digits_lc(b!("__45__.56"), 10, b'_'), (b!("__45"), b!("__.56")));
        assert_eq!(consume_digits_lc(b!("_4_5_"), 10, b'_'), (b!("_4"), b!("_5_")));
        assert_eq!(consume_digits_lc(b!("__4__5__"), 10, b'_'), (b!("__4"), b!("__5__")));
        assert_eq!(consume_digits_lc(b!("_4_5_.56"), 10, b'_'), (b!("_4"), b!("_5_.56")));
        assert_eq!(consume_digits_lc(b!("__4__5__.56"), 10, b'_'), (b!("__4"), b!("__5__.56")));

    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_i_test() {
        assert_eq!(consume_digits_i(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_i(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_i(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_i(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_i(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_i(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_i(b!("_.45"), 10, b'_'), (b!(""), b!("_.45")));
        assert_eq!(consume_digits_i(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_i(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_i(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_i(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_i(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_i(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_i(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_i(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_i(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_i(b!("_.45_5"), 10, b'_'), (b!(""), b!("_.45_5")));
        assert_eq!(consume_digits_i(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_i(b!("4_5_"), 10, b'_'), (b!("4_5"), b!("_")));
        assert_eq!(consume_digits_i(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_i(b!("4_5_.5"), 10, b'_'), (b!("4_5"), b!("_.5")));
        assert_eq!(consume_digits_i(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_i(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_i(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_i(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_i(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_i(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_i(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_i(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_i(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_ic(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_ic(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_ic(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_ic(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_ic(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_ic(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_ic(b!("_.45"), 10, b'_'), (b!(""), b!("_.45")));
        assert_eq!(consume_digits_ic(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_ic(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_ic(b!("4__5"), 10, b'_'), (b!("4__5"), b!("")));
        assert_eq!(consume_digits_ic(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_ic(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_ic(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_ic(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_ic(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_ic(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_ic(b!("_.45_5"), 10, b'_'), (b!(""), b!("_.45_5")));
        assert_eq!(consume_digits_ic(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_ic(b!("4_5_"), 10, b'_'), (b!("4_5"), b!("_")));
        assert_eq!(consume_digits_ic(b!("4__5__"), 10, b'_'), (b!("4__5"), b!("__")));
        assert_eq!(consume_digits_ic(b!("4_5_.5"), 10, b'_'), (b!("4_5"), b!("_.5")));
        assert_eq!(consume_digits_ic(b!("4__5__.5"), 10, b'_'), (b!("4__5"), b!("__.5")));
        assert_eq!(consume_digits_ic(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_ic(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_ic(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_ic(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_ic(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_ic(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_ic(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_ic(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_t_test() {
        assert_eq!(consume_digits_t(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_t(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_t(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_t(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_t(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_t(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_t(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_t(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_t(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_t(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_t(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_t(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_t(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_t(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_t(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_t(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_t(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_t(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_t(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_t(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_t(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_t(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_t(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_t(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_t(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_t(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_t(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_t(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_t(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_t(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_tc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_tc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_tc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_tc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_tc(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_tc(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_tc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_tc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_tc(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_tc(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_tc(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_tc(b!("4__"), 10, b'_'), (b!("4__"), b!("")));
        assert_eq!(consume_digits_tc(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_tc(b!("4__."), 10, b'_'), (b!("4__"), b!(".")));
        assert_eq!(consume_digits_tc(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_tc(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_tc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_tc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_tc(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_tc(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_tc(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_tc(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_tc(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_tc(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_tc(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_tc(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_tc(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_tc(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_tc(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_tc(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));
    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_il_test() {
        assert_eq!(consume_digits_il(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_il(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_il(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_il(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_il(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_il(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_il(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_il(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_il(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_il(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_il(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_il(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_il(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_il(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_il(b!("_45_5"), 10, b'_'), (b!("_45_5"), b!("")));
        assert_eq!(consume_digits_il(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_il(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_il(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_il(b!("4_5_"), 10, b'_'), (b!("4_5"), b!("_")));
        assert_eq!(consume_digits_il(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_il(b!("4_5_.5"), 10, b'_'), (b!("4_5"), b!("_.5")));
        assert_eq!(consume_digits_il(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_il(b!("_45_"), 10, b'_'), (b!("_45"), b!("_")));
        assert_eq!(consume_digits_il(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_il(b!("_45_.56"), 10, b'_'), (b!("_45"), b!("_.56")));
        assert_eq!(consume_digits_il(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_il(b!("_4_5_"), 10, b'_'), (b!("_4_5"), b!("_")));
        assert_eq!(consume_digits_il(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_il(b!("_4_5_.56"), 10, b'_'), (b!("_4_5"), b!("_.56")));
        assert_eq!(consume_digits_il(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_ilc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_ilc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_ilc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_ilc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_ilc(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_ilc(b!("__45"), 10, b'_'), (b!("__45"), b!("")));
        assert_eq!(consume_digits_ilc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_ilc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_ilc(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_ilc(b!("4__5"), 10, b'_'), (b!("4__5"), b!("")));
        assert_eq!(consume_digits_ilc(b!("4_"), 10, b'_'), (b!("4"), b!("_")));
        assert_eq!(consume_digits_ilc(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_ilc(b!("4_."), 10, b'_'), (b!("4"), b!("_.")));
        assert_eq!(consume_digits_ilc(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_ilc(b!("_45_5"), 10, b'_'), (b!("_45_5"), b!("")));
        assert_eq!(consume_digits_ilc(b!("__45__5"), 10, b'_'), (b!("__45__5"), b!("")));
        assert_eq!(consume_digits_ilc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_ilc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_ilc(b!("4_5_"), 10, b'_'), (b!("4_5"), b!("_")));
        assert_eq!(consume_digits_ilc(b!("4__5__"), 10, b'_'), (b!("4__5"), b!("__")));
        assert_eq!(consume_digits_ilc(b!("4_5_.5"), 10, b'_'), (b!("4_5"), b!("_.5")));
        assert_eq!(consume_digits_ilc(b!("4__5__.5"), 10, b'_'), (b!("4__5"), b!("__.5")));
        assert_eq!(consume_digits_ilc(b!("_45_"), 10, b'_'), (b!("_45"), b!("_")));
        assert_eq!(consume_digits_ilc(b!("__45__"), 10, b'_'), (b!("__45"), b!("__")));
        assert_eq!(consume_digits_ilc(b!("_45_.56"), 10, b'_'), (b!("_45"), b!("_.56")));
        assert_eq!(consume_digits_ilc(b!("__45__.56"), 10, b'_'), (b!("__45"), b!("__.56")));
        assert_eq!(consume_digits_ilc(b!("_4_5_"), 10, b'_'), (b!("_4_5"), b!("_")));
        assert_eq!(consume_digits_ilc(b!("__4__5__"), 10, b'_'), (b!("__4__5"), b!("__")));
        assert_eq!(consume_digits_ilc(b!("_4_5_.56"), 10, b'_'), (b!("_4_5"), b!("_.56")));
        assert_eq!(consume_digits_ilc(b!("__4__5__.56"), 10, b'_'), (b!("__4__5"), b!("__.56")));
    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_it_test() {
        assert_eq!(consume_digits_it(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_it(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_it(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_it(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_it(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_it(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_it(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_it(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_it(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_it(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_it(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_it(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_it(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_it(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_it(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_it(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_it(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_it(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_it(b!("4_5_"), 10, b'_'), (b!("4_5_"), b!("")));
        assert_eq!(consume_digits_it(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_it(b!("4_5_.5"), 10, b'_'), (b!("4_5_"), b!(".5")));
        assert_eq!(consume_digits_it(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_it(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_it(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_it(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_it(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_it(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_it(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_it(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_it(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_itc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_itc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_itc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_itc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_itc(b!("_45"), 10, b'_'), (b!(""), b!("_45")));
        assert_eq!(consume_digits_itc(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_itc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_itc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_itc(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_itc(b!("4__5"), 10, b'_'), (b!("4__5"), b!("")));
        assert_eq!(consume_digits_itc(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_itc(b!("4__"), 10, b'_'), (b!("4__"), b!("")));
        assert_eq!(consume_digits_itc(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_itc(b!("4__."), 10, b'_'), (b!("4__"), b!(".")));
        assert_eq!(consume_digits_itc(b!("_45_5"), 10, b'_'), (b!(""), b!("_45_5")));
        assert_eq!(consume_digits_itc(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_itc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_itc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_itc(b!("4_5_"), 10, b'_'), (b!("4_5_"), b!("")));
        assert_eq!(consume_digits_itc(b!("4__5__"), 10, b'_'), (b!("4__5__"), b!("")));
        assert_eq!(consume_digits_itc(b!("4_5_.5"), 10, b'_'), (b!("4_5_"), b!(".5")));
        assert_eq!(consume_digits_itc(b!("4__5__.5"), 10, b'_'), (b!("4__5__"), b!(".5")));
        assert_eq!(consume_digits_itc(b!("_45_"), 10, b'_'), (b!(""), b!("_45_")));
        assert_eq!(consume_digits_itc(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_itc(b!("_45_.56"), 10, b'_'), (b!(""), b!("_45_.56")));
        assert_eq!(consume_digits_itc(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_itc(b!("_4_5_"), 10, b'_'), (b!(""), b!("_4_5_")));
        assert_eq!(consume_digits_itc(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_itc(b!("_4_5_.56"), 10, b'_'), (b!(""), b!("_4_5_.56")));
        assert_eq!(consume_digits_itc(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));
    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_lt_test() {
        assert_eq!(consume_digits_lt(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_lt(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_lt(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_lt(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_lt(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_lt(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_lt(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_lt(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_lt(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_lt(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_lt(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_lt(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_lt(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_lt(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_lt(b!("_45_5"), 10, b'_'), (b!("_45"), b!("_5")));
        assert_eq!(consume_digits_lt(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_lt(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_lt(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_lt(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_lt(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_lt(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_lt(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_lt(b!("_45_"), 10, b'_'), (b!("_45_"), b!("")));
        assert_eq!(consume_digits_lt(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_lt(b!("_45_.56"), 10, b'_'), (b!("_45_"), b!(".56")));
        assert_eq!(consume_digits_lt(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_lt(b!("_4_5_"), 10, b'_'), (b!("_4"), b!("_5_")));
        assert_eq!(consume_digits_lt(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_lt(b!("_4_5_.56"), 10, b'_'), (b!("_4"), b!("_5_.56")));
        assert_eq!(consume_digits_lt(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_ltc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_ltc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_ltc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_ltc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_ltc(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_ltc(b!("__45"), 10, b'_'), (b!("__45"), b!("")));
        assert_eq!(consume_digits_ltc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_ltc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_ltc(b!("4_5"), 10, b'_'), (b!("4"), b!("_5")));
        assert_eq!(consume_digits_ltc(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_ltc(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_ltc(b!("4__"), 10, b'_'), (b!("4__"), b!("")));
        assert_eq!(consume_digits_ltc(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_ltc(b!("4__."), 10, b'_'), (b!("4__"), b!(".")));
        assert_eq!(consume_digits_ltc(b!("_45_5"), 10, b'_'), (b!("_45"), b!("_5")));
        assert_eq!(consume_digits_ltc(b!("__45__5"), 10, b'_'), (b!("__45"), b!("__5")));
        assert_eq!(consume_digits_ltc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_ltc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_ltc(b!("4_5_"), 10, b'_'), (b!("4"), b!("_5_")));
        assert_eq!(consume_digits_ltc(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_ltc(b!("4_5_.5"), 10, b'_'), (b!("4"), b!("_5_.5")));
        assert_eq!(consume_digits_ltc(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_ltc(b!("_45_"), 10, b'_'), (b!("_45_"), b!("")));
        assert_eq!(consume_digits_ltc(b!("__45__"), 10, b'_'), (b!("__45__"), b!("")));
        assert_eq!(consume_digits_ltc(b!("_45_.56"), 10, b'_'), (b!("_45_"), b!(".56")));
        assert_eq!(consume_digits_ltc(b!("__45__.56"), 10, b'_'), (b!("__45__"), b!(".56")));
        assert_eq!(consume_digits_ltc(b!("_4_5_"), 10, b'_'), (b!("_4"), b!("_5_")));
        assert_eq!(consume_digits_ltc(b!("__4__5__"), 10, b'_'), (b!("__4"), b!("__5__")));
        assert_eq!(consume_digits_ltc(b!("_4_5_.56"), 10, b'_'), (b!("_4"), b!("_5_.56")));
        assert_eq!(consume_digits_ltc(b!("__4__5__.56"), 10, b'_'), (b!("__4"), b!("__5__.56")));
    }

    #[cfg(feature = "format")]
    #[test]
    fn consume_digits_ilt_test() {
        assert_eq!(consume_digits_ilt(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_ilt(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_ilt(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_ilt(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_ilt(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_ilt(b!("__45"), 10, b'_'), (b!(""), b!("__45")));
        assert_eq!(consume_digits_ilt(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_ilt(b!("4__5"), 10, b'_'), (b!("4"), b!("__5")));
        assert_eq!(consume_digits_ilt(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_ilt(b!("__.45"), 10, b'_'), (b!(""), b!("__.45")));
        assert_eq!(consume_digits_ilt(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_ilt(b!("4__"), 10, b'_'), (b!("4"), b!("__")));
        assert_eq!(consume_digits_ilt(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_ilt(b!("4__."), 10, b'_'), (b!("4"), b!("__.")));
        assert_eq!(consume_digits_ilt(b!("_45_5"), 10, b'_'), (b!("_45_5"), b!("")));
        assert_eq!(consume_digits_ilt(b!("__45__5"), 10, b'_'), (b!(""), b!("__45__5")));
        assert_eq!(consume_digits_ilt(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_ilt(b!("__.45__5"), 10, b'_'), (b!(""), b!("__.45__5")));
        assert_eq!(consume_digits_ilt(b!("4_5_"), 10, b'_'), (b!("4_5_"), b!("")));
        assert_eq!(consume_digits_ilt(b!("4__5__"), 10, b'_'), (b!("4"), b!("__5__")));
        assert_eq!(consume_digits_ilt(b!("4_5_.5"), 10, b'_'), (b!("4_5_"), b!(".5")));
        assert_eq!(consume_digits_ilt(b!("4__5__.5"), 10, b'_'), (b!("4"), b!("__5__.5")));
        assert_eq!(consume_digits_ilt(b!("_45_"), 10, b'_'), (b!("_45_"), b!("")));
        assert_eq!(consume_digits_ilt(b!("__45__"), 10, b'_'), (b!(""), b!("__45__")));
        assert_eq!(consume_digits_ilt(b!("_45_.56"), 10, b'_'), (b!("_45_"), b!(".56")));
        assert_eq!(consume_digits_ilt(b!("__45__.56"), 10, b'_'), (b!(""), b!("__45__.56")));
        assert_eq!(consume_digits_ilt(b!("_4_5_"), 10, b'_'), (b!("_4_5_"), b!("")));
        assert_eq!(consume_digits_ilt(b!("__4__5__"), 10, b'_'), (b!(""), b!("__4__5__")));
        assert_eq!(consume_digits_ilt(b!("_4_5_.56"), 10, b'_'), (b!("_4_5_"), b!(".56")));
        assert_eq!(consume_digits_ilt(b!("__4__5__.56"), 10, b'_'), (b!(""), b!("__4__5__.56")));

        assert_eq!(consume_digits_iltc(b!("123.45"), 10, b'_'), (b!("123"), b!(".45")));
        assert_eq!(consume_digits_iltc(b!("1e45"), 10, b'_'), (b!("1"), b!("e45")));
        assert_eq!(consume_digits_iltc(b!("1e"), 10, b'_'), (b!("1"), b!("e")));
        assert_eq!(consume_digits_iltc(b!("1"), 10, b'_'), (b!("1"), b!("")));
        assert_eq!(consume_digits_iltc(b!("_45"), 10, b'_'), (b!("_45"), b!("")));
        assert_eq!(consume_digits_iltc(b!("__45"), 10, b'_'), (b!("__45"), b!("")));
        assert_eq!(consume_digits_iltc(b!("_.45"), 10, b'_'), (b!("_"), b!(".45")));
        assert_eq!(consume_digits_iltc(b!("__.45"), 10, b'_'), (b!("__"), b!(".45")));
        assert_eq!(consume_digits_iltc(b!("4_5"), 10, b'_'), (b!("4_5"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4__5"), 10, b'_'), (b!("4__5"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4_"), 10, b'_'), (b!("4_"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4__"), 10, b'_'), (b!("4__"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4_."), 10, b'_'), (b!("4_"), b!(".")));
        assert_eq!(consume_digits_iltc(b!("4__."), 10, b'_'), (b!("4__"), b!(".")));
        assert_eq!(consume_digits_iltc(b!("_45_5"), 10, b'_'), (b!("_45_5"), b!("")));
        assert_eq!(consume_digits_iltc(b!("__45__5"), 10, b'_'), (b!("__45__5"), b!("")));
        assert_eq!(consume_digits_iltc(b!("_.45_5"), 10, b'_'), (b!("_"), b!(".45_5")));
        assert_eq!(consume_digits_iltc(b!("__.45__5"), 10, b'_'), (b!("__"), b!(".45__5")));
        assert_eq!(consume_digits_iltc(b!("4_5_"), 10, b'_'), (b!("4_5_"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4__5__"), 10, b'_'), (b!("4__5__"), b!("")));
        assert_eq!(consume_digits_iltc(b!("4_5_.5"), 10, b'_'), (b!("4_5_"), b!(".5")));
        assert_eq!(consume_digits_iltc(b!("4__5__.5"), 10, b'_'), (b!("4__5__"), b!(".5")));
        assert_eq!(consume_digits_iltc(b!("_45_"), 10, b'_'), (b!("_45_"), b!("")));
        assert_eq!(consume_digits_iltc(b!("__45__"), 10, b'_'), (b!("__45__"), b!("")));
        assert_eq!(consume_digits_iltc(b!("_45_.56"), 10, b'_'), (b!("_45_"), b!(".56")));
        assert_eq!(consume_digits_iltc(b!("__45__.56"), 10, b'_'), (b!("__45__"), b!(".56")));
        assert_eq!(consume_digits_iltc(b!("_4_5_"), 10, b'_'), (b!("_4_5_"), b!("")));
        assert_eq!(consume_digits_iltc(b!("__4__5__"), 10, b'_'), (b!("__4__5__"), b!("")));
        assert_eq!(consume_digits_iltc(b!("_4_5_.56"), 10, b'_'), (b!("_4_5_"), b!(".56")));
        assert_eq!(consume_digits_iltc(b!("__4__5__.56"), 10, b'_'), (b!("__4__5__"), b!(".56")));
    }
}
