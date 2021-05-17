//! Utilities to parse, extract, and interpret exponent components.

use crate::atoi;
use crate::lib::slice;
use crate::util::*;
use super::traits::*;

/// The actual float-type doesn't matter, it just needs to be used for
/// signed/unsigned detection during sign parsing.
type FloatType = f64;

// EXPONENT CALCULATION

// Calculate the scientific notation exponent without overflow.
//
// For example, 0.1 would be -1, and 10 would be 1 in base 10.
perftools_inline!{
#[cfg(feature = "correct")]
pub(super) fn scientific_exponent(exponent: i32, integer_digits: usize, fraction_start: usize)
    -> i32
{
    if integer_digits == 0 {
        let fraction_start = fraction_start.try_i32_or_max();
        exponent.saturating_sub(fraction_start).saturating_sub(1)
    } else {
        let integer_shift = (integer_digits - 1).try_i32_or_max();
        exponent.saturating_add(integer_shift)
    }
}}

// Calculate the mantissa exponent without overflow.
//
// Remove the number of digits that contributed to the mantissa past
// the dot, and add the number of truncated digits from the mantissa,
// to calculate the scaling factor for the mantissa from a raw exponent.
perftools_inline!{
#[cfg(feature = "correct")]
pub(super) fn mantissa_exponent(raw_exponent: i32, fraction_digits: usize, truncated: usize)
    -> i32
{
    if fraction_digits > truncated {
        raw_exponent.saturating_sub((fraction_digits - truncated).try_i32_or_max())
    } else {
        raw_exponent.saturating_add((truncated - fraction_digits).try_i32_or_max())
    }
}}

// EXPONENT EXTRACTORS

// Extract exponent substring and parse exponent.
// Uses an abstract iterator to allow generic implementations
// iterators to work. This only works with greedy iterators, where we
// know exactly when we should stop upon encountering a given character.
//
// Precondition:
//      Iter should not implement ConsumedIterator, since it would break
//      the assumption in `extract_exponent_iltc`.
perftools_inline!{
#[allow(unused_unsafe)]
fn extract_and_parse_exponent<'a, Data, Iter>(
    data: &mut Data,
    iter: Iter,
    bytes: &'a [u8],
    radix: u32,
    sign: Sign
)
    -> &'a [u8]
    where Data: FastDataInterface<'a>,
          Iter: AsPtrIterator<'a, u8>
{
    let (raw_exponent, ptr) = atoi::standalone_exponent(iter, radix, sign);
    data.set_raw_exponent(raw_exponent);

    unsafe {
        // Extract the exponent subslice.
        let first = bytes.as_ptr();
        data.set_exponent(Some(slice::from_raw_parts(first, distance(first, ptr))));

        // Return the remaining bytes.
        let last = index!(bytes[bytes.len()..]).as_ptr();
        slice::from_raw_parts(ptr, distance(ptr, last))
    }
}}

// Parse exponent.
// This only works with exponents that may contain digit separators,
// where the invalid (trailing) data has already been determined.
perftools_inline!{
#[cfg(feature = "format")]
fn parse_exponent<'a, Data>(
    data: &mut Data,
    bytes: &'a [u8],
    leading: &'a [u8],
    trailing: &'a [u8],
    radix: u32,
    digit_separator: u8,
    sign: Sign
)
    where Data: FastDataInterface<'a>
{
    // Get an iterator over our digits and sign bits, and parse the exponent.
    let iter = iterate_digits_ignore_separator(leading, digit_separator);

    // Parse the exponent and store the extracted digits.
    let bytes_len = bytes.len() - trailing.len();
    data.set_raw_exponent(atoi::standalone_exponent(iter, radix, sign).0);
    data.set_exponent(Some(&index!(bytes[..bytes_len])));
}}

// PARSE THEN EXTRACT

// These algorithms are slightly more efficient, since they only
// require a single pass of the exponent string. These algorithms
// must be able to parse until they reach an invalid character,
// without any backsteps to find the correct subslice, that is,
// they must be greedy.

// Extract exponent substring and parse exponent.
// Does not consume any digit separators.
perftools_inline!{
fn extract_exponent<'a, Data>(
    data: &mut Data,
    bytes: &'a [u8],
    radix: u32,
    digit_separator: u8
)
    -> &'a [u8]
    where Data: FastDataInterface<'a>
{
    // Remove leading exponent character and parse exponent.
    let bytes = &index!(bytes[1..]);
    let (sign, digits) = parse_sign_no_separator::<FloatType>(bytes, digit_separator);
    let iter = iterate_digits_no_separator(digits, digit_separator);
    extract_and_parse_exponent(data, iter, bytes, radix, sign)
}}

// Extract exponent substring and parse exponent.
// Consumes leading, internal, trailing, and consecutive digit separators.
perftools_inline!{
#[cfg(feature = "format")]
fn extract_exponent_iltc<'a, Data>(
    data: &mut Data,
    bytes: &'a [u8],
    radix: u32,
    digit_separator: u8
)
    -> &'a [u8]
    where Data: FastDataInterface<'a>
{
    // Remove leading exponent character and parse exponent.
    // We're not calling `consumed()`, so it's fine to have trailing underscores.
    let bytes = &index!(bytes[1..]);
    let (sign, digits) = parse_sign_lc_separator::<FloatType>(bytes, digit_separator);
    let iter = iterate_digits_ignore_separator(digits, digit_separator);
    extract_and_parse_exponent(data, iter, bytes, radix, sign)
}}

// EXTRACT THEN PARSE

// These algorithms are less efficient, since they first extract the
// subslice of valid data in the exponent, and then parse it,
// using 2 passes over the input data. However, because they first extract
// the data, they allow consumers that are not greedy, where there may
// be backsteps to determine if an input is actually valid after reaching
// the end or an invalid character.

// Generate function definition to extraction exponent with digit separators.
macro_rules! extract_exponent_separator {
    (
        fn $name:ident,
        sign => $sign:ident,
        consume => $consume:ident
    ) => (
        perftools_inline!{
        #[cfg(feature = "format")]
        fn $name<'a, Data>(
            data: &mut Data,
            bytes: &'a [u8],
            radix: u32,
            digit_separator: u8
        )
            -> &'a [u8]
            where Data: FastDataInterface<'a>
        {
            let bytes = &index!(bytes[1..]);
            let (sign, digits) = $sign::<FloatType>(bytes, digit_separator);
            let (leading, trailing) = $consume(digits, radix, digit_separator);
            parse_exponent(data, bytes, leading, trailing, radix, digit_separator, sign);

            trailing
        }}
    );
}

extract_exponent_separator!(
    fn extract_exponent_i,
    sign => parse_sign_no_separator,
    consume => consume_digits_i
);

extract_exponent_separator!(
    fn extract_exponent_ic,
    sign => parse_sign_no_separator,
    consume => consume_digits_ic
);

extract_exponent_separator!(
    fn extract_exponent_l,
    sign => parse_sign_l_separator,
    consume => consume_digits_l
);

extract_exponent_separator!(
    fn extract_exponent_lc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_lc
);

extract_exponent_separator!(
    fn extract_exponent_t,
    sign => parse_sign_no_separator,
    consume => consume_digits_t
);

extract_exponent_separator!(
    fn extract_exponent_tc,
    sign => parse_sign_no_separator,
    consume => consume_digits_tc
);

extract_exponent_separator!(
    fn extract_exponent_il,
    sign => parse_sign_l_separator,
    consume => consume_digits_il
);

extract_exponent_separator!(
    fn extract_exponent_ilc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ilc
);

extract_exponent_separator!(
    fn extract_exponent_it,
    sign => parse_sign_no_separator,
    consume => consume_digits_it
);

extract_exponent_separator!(
    fn extract_exponent_itc,
    sign => parse_sign_no_separator,
    consume => consume_digits_itc
);

extract_exponent_separator!(
    fn extract_exponent_lt,
    sign => parse_sign_l_separator,
    consume => consume_digits_lt
);

extract_exponent_separator!(
    fn extract_exponent_ltc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ltc
);

extract_exponent_separator!(
    fn extract_exponent_ilt,
    sign => parse_sign_l_separator,
    consume => consume_digits_ilt
);

// API

// Extract exponent without a digit separator.
perftools_inline!{
pub(crate) fn extract_exponent_no_separator<'a, Data>(data: &mut Data, bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> &'a [u8]
    where Data: FastDataInterface<'a>
{
    extract_exponent(data, bytes, radix, format.digit_separator())
}}

// Extract exponent while ignoring the digit separator.
perftools_inline!{
#[cfg(feature = "format")]
pub(crate) fn extract_exponent_ignore_separator<'a, Data>(data: &mut Data, bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> &'a [u8]
    where Data: FastDataInterface<'a>
{
    extract_exponent_iltc(data, bytes, radix, format.digit_separator())
}}

// Extract exponent with a digit separator in the exponent component.
perftools_inline!{
#[cfg(feature = "format")]
pub(super) fn extract_exponent_separator<'a, Data>(data: &mut Data, bytes: &'a [u8], radix: u32, format: NumberFormat)
    -> &'a [u8]
    where Data: FastDataInterface<'a>
{
    const I: NumberFormat = NumberFormat::EXPONENT_INTERNAL_DIGIT_SEPARATOR;
    const L: NumberFormat = NumberFormat::EXPONENT_LEADING_DIGIT_SEPARATOR;
    const T: NumberFormat = NumberFormat::EXPONENT_TRAILING_DIGIT_SEPARATOR;
    const C: NumberFormat = NumberFormat::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR;
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
    match format & NumberFormat::EXPONENT_DIGIT_SEPARATOR_FLAG_MASK {
        I       => extract_exponent_i(data, bytes, radix, digit_separator),
        IC      => extract_exponent_ic(data, bytes, radix, digit_separator),
        L       => extract_exponent_l(data, bytes, radix, digit_separator),
        LC      => extract_exponent_lc(data, bytes, radix, digit_separator),
        T       => extract_exponent_t(data, bytes, radix, digit_separator),
        TC      => extract_exponent_tc(data, bytes, radix, digit_separator),
        IL      => extract_exponent_il(data, bytes, radix, digit_separator),
        ILC     => extract_exponent_ilc(data, bytes, radix, digit_separator),
        IT      => extract_exponent_it(data, bytes, radix, digit_separator),
        ITC     => extract_exponent_itc(data, bytes, radix, digit_separator),
        LT      => extract_exponent_lt(data, bytes, radix, digit_separator),
        LTC     => extract_exponent_ltc(data, bytes, radix, digit_separator),
        ILT     => extract_exponent_ilt(data, bytes, radix, digit_separator),
        ILTC    => extract_exponent_iltc(data, bytes, radix, digit_separator),
        _       => unreachable!()
    }
}}

// TESTS
// -----

#[cfg(test)]
mod test {
    use super::*;
    use super::super::standard::*;

    #[cfg(feature = "format")]
    use super::super::ignore::*;

    #[cfg(feature = "correct")]
    #[test]
    fn scientific_exponent_test() {
        // 0 digits in the integer
        assert_eq!(scientific_exponent(0, 0, 5), -6);
        assert_eq!(scientific_exponent(10, 0, 5), 4);
        assert_eq!(scientific_exponent(-10, 0, 5), -16);

        // >0 digits in the integer
        assert_eq!(scientific_exponent(0, 1, 5), 0);
        assert_eq!(scientific_exponent(0, 2, 5), 1);
        assert_eq!(scientific_exponent(0, 2, 20), 1);
        assert_eq!(scientific_exponent(10, 2, 20), 11);
        assert_eq!(scientific_exponent(-10, 2, 20), -9);

        // Underflow
        assert_eq!(scientific_exponent(i32::min_value(), 0, 0), i32::min_value());
        assert_eq!(scientific_exponent(i32::min_value(), 0, 5), i32::min_value());

        // Overflow
        assert_eq!(scientific_exponent(i32::max_value(), 0, 0), i32::max_value()-1);
        assert_eq!(scientific_exponent(i32::max_value(), 5, 0), i32::max_value());
    }

    #[cfg(feature = "correct")]
    #[test]
    fn mantissa_exponent_test() {
        assert_eq!(mantissa_exponent(10, 5, 0), 5);
        assert_eq!(mantissa_exponent(0, 5, 0), -5);
        assert_eq!(mantissa_exponent(i32::max_value(), 5, 0), i32::max_value()-5);
        assert_eq!(mantissa_exponent(i32::max_value(), 0, 5), i32::max_value());
        assert_eq!(mantissa_exponent(i32::min_value(), 5, 0), i32::min_value());
        assert_eq!(mantissa_exponent(i32::min_value(), 0, 5), i32::min_value()+5);
    }

    #[test]
    fn extract_exponent_test() {
        // Allows present exponents.
        type Data<'a> = StandardFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::standard().unwrap());
        extract_exponent(&mut data, b"e+23", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::standard().unwrap());
        extract_exponent(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_iltc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_iltc(&mut data, b"e__+__2__3____", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("__+__2__3____")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows present exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_iltc(&mut data, b"e__+_2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("__+_2_3_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows present exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_iltc(&mut data, b"e_+__2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+__2_3_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_iltc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_i_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_i(&mut data, b"e+2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2_3")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_i(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_i(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_i(&mut data, b"e+2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2")));
        assert_eq!(data.raw_exponent(), 2);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_ic_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ic(&mut data, b"e+2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2__3")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ic(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ic(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ic(&mut data, b"e_+2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_l_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_l(&mut data, b"e+_23", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+_23")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_l(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_l(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+_2")));
        assert_eq!(data.raw_exponent(), 2);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_l(&mut data, b"e_+__2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_lc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lc(&mut data, b"e+__23", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+__23")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lc(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+_2")));
        assert_eq!(data.raw_exponent(), 2);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lc(&mut data, b"e_+__2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+__2")));
        assert_eq!(data.raw_exponent(), 2);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_t_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_t(&mut data, b"e+23_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_t(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_t(&mut data, b"e+23__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23")));
        assert_eq!(data.raw_exponent(), 23);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_t(&mut data, b"e_+__2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_tc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_tc(&mut data, b"e+23__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23__")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_tc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_tc(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_tc(&mut data, b"e_+__2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_il_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_il(&mut data, b"e+_2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+_2_3")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_il(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_il(&mut data, b"e+23__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23")));
        assert_eq!(data.raw_exponent(), 23);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_il(&mut data, b"e+2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2")));
        assert_eq!(data.raw_exponent(), 2);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_ilc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilc(&mut data, b"e+__2__3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+__2__3")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilc(&mut data, b"e+23__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+23")));
        assert_eq!(data.raw_exponent(), 23);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilc(&mut data, b"e+2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2__3")));
        assert_eq!(data.raw_exponent(), 23);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_it_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_it(&mut data, b"e+2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2_3_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_it(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_it(&mut data, b"e+_23", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_it(&mut data, b"e+2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2")));
        assert_eq!(data.raw_exponent(), 2);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_itc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_itc(&mut data, b"e+2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2__3__")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_itc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_itc(&mut data, b"e+_23", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_itc(&mut data, b"e_+2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_")));
        assert_eq!(data.raw_exponent(), 0);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_lt_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lt(&mut data, b"e_+_23_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+_23_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lt(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lt(&mut data, b"e+2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2")));
        assert_eq!(data.raw_exponent(), 2);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_lt(&mut data, b"e__+__2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_ltc_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ltc(&mut data, b"e__+__23__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("__+__23__")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ltc(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ltc(&mut data, b"e+2_3", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("+2")));
        assert_eq!(data.raw_exponent(), 2);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ltc(&mut data, b"e__+__2__3__", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("__+__2")));
        assert_eq!(data.raw_exponent(), 2);
    }

    #[test]
    #[cfg(feature = "format")]
    fn extract_exponent_ilt_test() {
        // Allows present exponents.
        type Data<'a> = IgnoreFastDataInterface<'a>;
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilt(&mut data, b"e_+_2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+_2_3_")));
        assert_eq!(data.raw_exponent(), 23);

        // Allows absent exponents.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilt(&mut data, b"e", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));
        assert_eq!(data.raw_exponent(), 0);

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilt(&mut data, b"e__+_2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("")));

        // Ignores invalid data.
        let mut data = Data::new(NumberFormat::ignore(b'_').unwrap());
        extract_exponent_ilt(&mut data, b"e_+__2_3_", 10, b'_');
        assert_eq!(data.exponent(), Some(b!("_+")));
    }
}
