//! Fast, generic, lexical string-to-integer conversion routines.

//  The following benchmarks were run on an "Intel(R) Core(TM) i7-6560U
//  CPU @ 2.20GHz" CPU, on Fedora 28, Linux kernel version 4.18.16-200
//  (x86-64), using the lexical formatter or `x.parse()`,
//  avoiding any inefficiencies in Rust string parsing. The code was
//  compiled with LTO and at an optimization level of 3.
//
//  The benchmarks with `std` were compiled using "rustc 1.32.0
// (9fda7c223 2019-01-16)".
//
//  The benchmark code may be found `benches/atoi.rs`.
//
//  # Benchmarks
//
//  | Type  |  lexical (ns/iter) | parse (ns/iter)       | Relative Increase |
//  |:-----:|:------------------:|:---------------------:|:-----------------:|
//  | u8    | 75,622             | 80,021                | 1.06x             |
//  | u16   | 80,926             | 82,185                | 1.02x             |
//  | u32   | 131,221            | 148,231               | 1.13x             |
//  | u64   | 243,315            | 296,713               | 1.22x             |
//  | u128  | 512,552            | 1,175,946             | 2.29x             |
//  | i8    | 112,152            | 115,147               | 1.03x             |
//  | i16   | 153,670            | 150,231               | 0.98x             |
//  | i32   | 202,512            | 204,880               | 1.01x             |
//  | i64   | 309,731            | 309,584               | 1.00x             |
//  | i128  | 4,362,672          | 149,418,085           | 34.3x             |
//
//  # Raw Benchmarks
//
//  ```text
//  test atoi_i128_lexical        ... bench:   4,305,621 ns/iter (+/- 132,707)
//  test atoi_i128_parse          ... bench: 146,893,478 ns/iter (+/- 2,822,002)
//  test atoi_i16_lexical         ... bench:     132,255 ns/iter (+/- 5,503)
//  test atoi_i16_parse           ... bench:     137,965 ns/iter (+/- 5,906)
//  test atoi_i32_lexical         ... bench:     207,101 ns/iter (+/- 79,541)
//  test atoi_i32_parse           ... bench:     194,225 ns/iter (+/- 9,065)
//  test atoi_i64_lexical         ... bench:     271,538 ns/iter (+/- 9,137)
//  test atoi_i64_parse           ... bench:     293,542 ns/iter (+/- 9,706)
//  test atoi_i8_lexical          ... bench:     106,368 ns/iter (+/- 5,919)
//  test atoi_i8_parse            ... bench:     108,316 ns/iter (+/- 3,418)
//  test atoi_u128_lexical        ... bench:     496,426 ns/iter (+/- 40,197)
//  test atoi_u128_parse          ... bench:   1,119,213 ns/iter (+/- 54,945)
//  test atoi_u128_simple_lexical ... bench:     121,121 ns/iter (+/- 4,858)
//  test atoi_u128_simple_parse   ... bench:      97,518 ns/iter (+/- 2,739)
//  test atoi_u16_lexical         ... bench:      80,886 ns/iter (+/- 2,366)
//  test atoi_u16_parse           ... bench:      81,881 ns/iter (+/- 1,708)
//  test atoi_u16_simple_lexical  ... bench:      62,819 ns/iter (+/- 1,707)
//  test atoi_u16_simple_parse    ... bench:      60,916 ns/iter (+/- 8,340)
//  test atoi_u32_lexical         ... bench:     139,264 ns/iter (+/- 3,945)
//  test atoi_u32_parse           ... bench:     139,649 ns/iter (+/- 5,735)
//  test atoi_u32_simple_lexical  ... bench:      61,398 ns/iter (+/- 1,248)
//  test atoi_u32_simple_parse    ... bench:      59,560 ns/iter (+/- 3,388)
//  test atoi_u64_lexical         ... bench:     257,116 ns/iter (+/- 6,810)
//  test atoi_u64_parse           ... bench:     273,811 ns/iter (+/- 6,871)
//  test atoi_u64_simple_lexical  ... bench:      59,674 ns/iter (+/- 4,852)
//  test atoi_u64_simple_parse    ... bench:      55,982 ns/iter (+/- 2,288)
//  test atoi_u8_lexical          ... bench:      70,637 ns/iter (+/- 1,889)
//  test atoi_u8_parse            ... bench:      67,606 ns/iter (+/- 1,924)
//  test atoi_u8_simple_lexical   ... bench:      41,190 ns/iter (+/- 6,948)
//  test atoi_u8_simple_parse     ... bench:      36,836 ns/iter (+/- 2,958)
//  ```
//
// Code the generate the benchmark plot:
//  import numpy as np
//  import pandas as pd
//  import matplotlib.pyplot as plt
//  plt.style.use('ggplot')
//  lexical = np.array([75622, 80926, 131221, 243315, 512552, 112152, 153670, 202512, 309731, 4362672]) / 1e6
//  rustcore = np.array([80021, 82185, 148231, 296713, 1175946, 115147, 150231, 204880, 309584, 149418085]) / 1e6
//  index = ["u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128"]
//  df = pd.DataFrame({'lexical': lexical, 'rustcore': rustcore}, index = index, columns=['lexical', 'rustcore'])
//  ax = df.plot.bar(rot=0, figsize=(16, 8), fontsize=14, color=['#E24A33', '#348ABD'])
//  ax.set_ylabel("ms/iter")
//  ax.set_yscale('log')
//  ax.figure.tight_layout()
//  ax.legend(loc=2, prop={'size': 14})
//  plt.show()

use crate::util::*;
use super::shared::*;

// SHARED
// ------

// Validate the extracted integer has no leading zeros.
perftools_inline!{
#[cfg(feature = "format")]
fn validate_no_leading_zeros<'a>(digits: &[u8], digit_separator: u8, ptr: *const u8)
    -> ParseResult<()>
{
    // Check if the next character is a sign symbol.
    let index = distance(digits.as_ptr(), ptr);
    let digits = &index!(digits[..index]);
    let mut iter = iterate_digits_ignore_separator(digits, digit_separator);
    let is_zero = match iter.next() {
        Some(&b'+') | Some(&b'-')   => false,
        Some(&b'0')                 => true,
        _                           => return Ok(())
    };

    // Only here if we have a leading 0 or sign symbol.
    match iter.next() {
        Some(_) if is_zero  => return Err((ErrorCode::InvalidLeadingZeros, digits.as_ptr())),
        Some(&b'0')         => (),
        _                   => return Ok(())
    }

    // Only here if we have a leading 0 symbol.
    match iter.next() {
        Some(_)             => Err((ErrorCode::InvalidLeadingZeros, digits.as_ptr())),
        _                   => Ok(())
    }
}}

// STANDALONE
// ----------

/// Iterate over the digits and iteratively process them.
macro_rules! parse_digits {
    ($value:ident, $iter:ident, $radix:ident, $op:ident, $code:ident) => (
        while let Some(c) = $iter.next() {
            let digit = match to_digit!(*c, $radix) {
                Some(v) => v,
                None    => return Ok(($value, c)),
            };
            $value = match $value.checked_mul(as_cast($radix)) {
                Some(v) => v,
                None    => return Err((ErrorCode::$code, c)),
            };
            $value = match $value.$op(as_cast(digit)) {
                Some(v) => v,
                None    => return Err((ErrorCode::$code, c)),
            };
        }
    );
}

// Parse the digits for the atoi processor.
perftools_inline_always!{
fn parse_digits<'a, T, Iter>(digits: &[u8], mut iter: Iter, radix: u32, sign: Sign)
    -> ParseResult<(T, *const u8)>
    where T: Integer,
          Iter: AsPtrIterator<'a, u8>
{
    let mut value = T::ZERO;
    if sign == Sign::Positive {
        parse_digits!(value, iter, radix, checked_add, Overflow);
    } else {
        parse_digits!(value, iter, radix, checked_sub, Underflow);
    }
    Ok((value, last_ptr(digits)))
}}

// PARSE THEN EXTRACT

// Standalone atoi processor without a digit separator.
perftools_inline_always!{
fn standalone<T>(bytes: &[u8], radix: u32)
    -> ParseResult<(T, *const u8)>
    where T: Integer
{
    let (sign, digits) = parse_sign!(bytes, T::IS_SIGNED, Empty);
    let iter = iterate_digits_no_separator(digits, b'\x00');
    parse_digits(digits, iter, radix, sign)
}}

// Standalone atoi processor with digit separators.
// Consumes leading, internal, trailing, and consecutive digit separators.
perftools_inline_always!{
#[cfg(feature = "format")]
fn standalone_iltc<T>(bytes: &[u8], radix: u32, digit_separator: u8)
    -> ParseResult<(T, *const u8)>
    where T: Integer
{
    let (sign, digits) = parse_sign_lc_separator::<T>(bytes, digit_separator);
    if digits.is_empty() {
        return Err((ErrorCode::Empty, digits.as_ptr()));
    }
    let iter = iterate_digits_ignore_separator(digits, digit_separator);
    parse_digits(digits, iter, radix, sign)
}}

// EXTRACT THEN PARSE

// Generate function definition to extract then parse an integer with digit separators.
macro_rules! standalone_atoi_separator {
    (
        fn $name:ident,
        sign => $sign:ident,
        consume => $consume:ident
    ) => (
        perftools_inline_always!{
        #[cfg(feature = "format")]
        fn $name<T>(
            bytes: &[u8],
            radix: u32,
            digit_separator: u8
        )
            -> ParseResult<(T, *const u8)>
            where T: Integer
        {
            // Parse the sign, and error if we have no more digits.
            let (sign, digits) = $sign::<T>(bytes, digit_separator);
            if digits.is_empty() {
                return Err((ErrorCode::Empty, digits.as_ptr()));
            }

            // Extract the integer subslice, then parse.
            let leading = $consume(digits, radix, digit_separator).0;
            let iter = iterate_digits_ignore_separator(leading, digit_separator);

            parse_digits(leading, iter, radix, sign)
        }}
    );
}

standalone_atoi_separator!(
    fn standalone_i,
    sign => parse_sign_no_separator,
    consume => consume_digits_i
);

standalone_atoi_separator!(
    fn standalone_ic,
    sign => parse_sign_no_separator,
    consume => consume_digits_ic
);

standalone_atoi_separator!(
    fn standalone_l,
    sign => parse_sign_l_separator,
    consume => consume_digits_l
);

standalone_atoi_separator!(
    fn standalone_lc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_lc
);

standalone_atoi_separator!(
    fn standalone_t,
    sign => parse_sign_no_separator,
    consume => consume_digits_t
);

standalone_atoi_separator!(
    fn standalone_tc,
    sign => parse_sign_no_separator,
    consume => consume_digits_tc
);

standalone_atoi_separator!(
    fn standalone_il,
    sign => parse_sign_l_separator,
    consume => consume_digits_il
);

standalone_atoi_separator!(
    fn standalone_ilc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ilc
);

standalone_atoi_separator!(
    fn standalone_it,
    sign => parse_sign_no_separator,
    consume => consume_digits_it
);

standalone_atoi_separator!(
    fn standalone_itc,
    sign => parse_sign_no_separator,
    consume => consume_digits_itc
);

standalone_atoi_separator!(
    fn standalone_lt,
    sign => parse_sign_l_separator,
    consume => consume_digits_lt
);

standalone_atoi_separator!(
    fn standalone_ltc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ltc
);

standalone_atoi_separator!(
    fn standalone_ilt,
    sign => parse_sign_l_separator,
    consume => consume_digits_ilt
);

// API

// Standalone atoi processor without a digit separator.
perftools_inline_always!{
pub(crate) fn standalone_no_separator<T>(bytes: &[u8], radix: u32)
    -> ParseResult<(T, *const u8)>
    where T: Integer
{
    standalone(bytes, radix)
}}

// Extract exponent with a digit separator in the exponent component.
perftools_inline_always!{
#[cfg(feature = "format")]
pub(crate) fn standalone_separator<V>(bytes: &[u8], radix: u32, format: NumberFormat)
    -> ParseResult<(V, *const u8)>
    where V: Integer
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
    let (value, ptr) = match format & NumberFormat::INTEGER_DIGIT_SEPARATOR_FLAG_MASK {
        I       => standalone_i(bytes, radix, digit_separator),
        IC      => standalone_ic(bytes, radix, digit_separator),
        L       => standalone_l(bytes, radix, digit_separator),
        LC      => standalone_lc(bytes, radix, digit_separator),
        T       => standalone_t(bytes, radix, digit_separator),
        TC      => standalone_tc(bytes, radix, digit_separator),
        IL      => standalone_il(bytes, radix, digit_separator),
        ILC     => standalone_ilc(bytes, radix, digit_separator),
        IT      => standalone_it(bytes, radix, digit_separator),
        ITC     => standalone_itc(bytes, radix, digit_separator),
        LT      => standalone_lt(bytes, radix, digit_separator),
        LTC     => standalone_ltc(bytes, radix, digit_separator),
        ILT     => standalone_ilt(bytes, radix, digit_separator),
        ILTC    => standalone_iltc(bytes, radix, digit_separator),
        // No digit separator match.
        _       => standalone(bytes, radix)
    }?;

    // Check if we have any leading zeros.
    if format.no_integer_leading_zeros() {
        validate_no_leading_zeros(bytes, digit_separator, ptr)?;
    }

    Ok((value, ptr))
}}

// STANDALONE U128
// ---------------

// Grab the step size and power for step_u64.
// This is the same as the u128 divisor, so don't duplicate the values
// there.
perftools_inline_always!{
fn step_u64(radix: u32) -> usize {
    u128_divisor(radix).1
}}

// Add 64-bit temporary to the 128-bit value.
macro_rules! add_temporary_128 {
    ($value:ident, $tmp:ident, $step_power:ident, $ptr:expr, $op:ident, $code:ident) => (
        if !$value.is_zero() {
            $value = match $value.checked_mul(as_cast($step_power)) {
                Some(v) => v,
                None    => return Err((ErrorCode::$code, $ptr)),
            };
        }
        $value = match $value.$op(as_cast($tmp)) {
            Some(v) => v,
            None    => return Err((ErrorCode::$code, $ptr)),
        };
    );
}

/// Iterate over the digits and iteratively process them.
macro_rules! parse_digits_u128 {
    ($value:ident, $iter:ident, $radix:ident, $step:ident, $op:ident, $code:ident) => ({
        // Break the input into chunks of len `step`, which can be parsed
        // as a 64-bit integer.
        while !$iter.consumed() {
            let mut value: u64 = 0;
            let mut index = 0;
            while index < $step {
                if let Some(c) = $iter.next() {
                    index += 1;
                    let digit = match to_digit!(*c, $radix) {
                        Some(v) => v,
                        None    => {
                            // Add temporary to value and return early.
                            let radix_pow = $radix.as_u64().pow(index.as_u32());
                            add_temporary_128!($value, value, radix_pow, c, $op, $code);
                            return Ok(($value, c));
                        },
                    };

                    // Don't have to worry about overflows.
                    value *= $radix.as_u64();
                    value += digit.as_u64();
                } else {
                    break;
                }
            }

            // Add the temporary value to the total value.
            let radix_pow = $radix.as_u64().pow(index.as_u32());
            add_temporary_128!($value, value, radix_pow, $iter.as_ptr(), $op, $code);
        }
    });
}

// Quickly parse digits using a 64-bit intermediate for the 128-bit atoi processor.
perftools_inline_always!{
fn parse_digits_128_fast<'a, W, N, Iter>(digits: &[u8], iter: Iter, radix: u32, sign: Sign)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer,
          Iter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>
{
    let (value, ptr) = parse_digits::<N, _>(digits, iter, radix, sign)?;
    Ok((as_cast(value), ptr))
}}

// Slowly parse digits for the 128-bit atoi processor.
perftools_inline_always!{
fn parse_digits_128_slow<'a, T, Iter>(digits: &[u8], mut iter: Iter, radix: u32, step: usize, sign: Sign)
    -> ParseResult<(T, *const u8)>
    where T: Integer,
          Iter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>
{
    let mut value = T::ZERO;
    if sign == Sign::Positive {
        parse_digits_u128!(value, iter, radix, step, checked_add, Overflow)
    } else {
        parse_digits_u128!(value, iter, radix, step, checked_sub, Underflow)
    }
    Ok((value, last_ptr(digits)))
}}

// Parse digits for the 128-bit atoi processor.
//
// This algorithm may overestimate the number of digits to overflow
// on numeric overflow or underflow, otherwise, it will be accurate.
// This is because we break costly u128 addition/multiplications into
// temporary steps using u64, allowing much better performance.
// This is a similar approach to what we take in the arbitrary-precision
// arithmetic.
perftools_inline_always!{
fn parse_digits_128<'a, W, N, Iter>(digits: &[u8], iter: Iter, radix: u32, sign: Sign)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer,
          Iter: ConsumedIterator<Item=&'a u8> + AsPtrIterator<'a, u8>
{
    // This is guaranteed to be safe, since if the length is
    // 1 less than step, and the min radix is 2, the value must be
    // less than 2x u64::MAX, which means it must fit in an i64.
    let step = step_u64(radix);
    if digits.len() < step {
        parse_digits_128_fast::<W, N, _>(digits, iter, radix, sign)
    } else {
        parse_digits_128_slow(digits, iter, radix, step, sign)
    }
}}

// PARSE THEN EXTRACT

// Standalone atoi processor for 128-bit integers without a digit separator.
perftools_inline_always!{
fn standalone_128<W, N>(bytes: &[u8], radix: u32)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer
{
    let (sign, digits) = parse_sign!(bytes, W::IS_SIGNED, Empty);
    let iter = iterate_digits_no_separator(digits, b'\x00');
    parse_digits_128::<W, N, _>(digits, iter, radix, sign)
}}

// Standalone atoi processor for 128-bit integers with digit separators.
// Consumes leading, internal, trailing, and consecutive digit separators.
perftools_inline_always!{
#[cfg(feature = "format")]
fn standalone_128_iltc<W, N>(bytes: &[u8], radix: u32, digit_separator: u8)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer
{
    let (sign, digits) = parse_sign_lc_separator::<W>(bytes, digit_separator);
    if digits.is_empty() {
        return Err((ErrorCode::Empty, digits.as_ptr()));
    }
    let iter = iterate_digits_ignore_separator(digits, digit_separator);
    parse_digits_128::<W, N, _>(digits, iter, radix, sign)
}}

// EXTRACT THEN PARSE

// Generate function definition to extract then parse an 128-bit integer with digit separators.
macro_rules! standalone_atoi_128_separator {
    (
        fn $name:ident,
        sign => $sign:ident,
        consume => $consume:ident
    ) => (
        perftools_inline!{
        #[cfg(feature = "format")]
        fn $name<W, N>(
            bytes: &[u8],
            radix: u32,
            digit_separator: u8
        )
            -> ParseResult<(W, *const u8)>
            where W: Integer,
                  N: Integer
        {
            // Parse the sign, and error if we have no more digits.
            let (sign, digits) = $sign::<W>(bytes, digit_separator);
            if digits.is_empty() {
                return Err((ErrorCode::Empty, digits.as_ptr()));
            }

            // Extract the integer subslice, then parse.
            let leading = $consume(digits, radix, digit_separator).0;
            let iter = iterate_digits_ignore_separator(leading, digit_separator);
            parse_digits_128::<W, N, _>(leading, iter, radix, sign)
        }}
    );
}

standalone_atoi_128_separator!(
    fn standalone_128_i,
    sign => parse_sign_no_separator,
    consume => consume_digits_i
);

standalone_atoi_128_separator!(
    fn standalone_128_ic,
    sign => parse_sign_no_separator,
    consume => consume_digits_ic
);

standalone_atoi_128_separator!(
    fn standalone_128_l,
    sign => parse_sign_l_separator,
    consume => consume_digits_l
);

standalone_atoi_128_separator!(
    fn standalone_128_lc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_lc
);

standalone_atoi_128_separator!(
    fn standalone_128_t,
    sign => parse_sign_no_separator,
    consume => consume_digits_t
);

standalone_atoi_128_separator!(
    fn standalone_128_tc,
    sign => parse_sign_no_separator,
    consume => consume_digits_tc
);

standalone_atoi_128_separator!(
    fn standalone_128_il,
    sign => parse_sign_l_separator,
    consume => consume_digits_il
);

standalone_atoi_128_separator!(
    fn standalone_128_ilc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ilc
);

standalone_atoi_128_separator!(
    fn standalone_128_it,
    sign => parse_sign_no_separator,
    consume => consume_digits_it
);

standalone_atoi_128_separator!(
    fn standalone_128_itc,
    sign => parse_sign_no_separator,
    consume => consume_digits_itc
);

standalone_atoi_128_separator!(
    fn standalone_128_lt,
    sign => parse_sign_l_separator,
    consume => consume_digits_lt
);

standalone_atoi_128_separator!(
    fn standalone_128_ltc,
    sign => parse_sign_lc_separator,
    consume => consume_digits_ltc
);

standalone_atoi_128_separator!(
    fn standalone_128_ilt,
    sign => parse_sign_l_separator,
    consume => consume_digits_ilt
);

// API

// Standalone atoi processor for u128 without a digit separator.
perftools_inline_always!{
pub(crate) fn standalone_128_no_separator<W, N>(bytes: &[u8], radix: u32)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer
{
    standalone_128::<W, N>(bytes, radix)
}}

// Extract exponent with a digit separator in the exponent component.
perftools_inline_always!{
#[cfg(feature = "format")]
pub(crate) fn standalone_128_separator<W, N>(bytes: &[u8], radix: u32, format: NumberFormat)
    -> ParseResult<(W, *const u8)>
    where W: Integer,
          N: Integer
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
    let (value, ptr) = match format & NumberFormat::INTEGER_DIGIT_SEPARATOR_FLAG_MASK {
        I       => standalone_128_i::<W, N>(bytes, radix, digit_separator),
        IC      => standalone_128_ic::<W, N>(bytes, radix, digit_separator),
        L       => standalone_128_l::<W, N>(bytes, radix, digit_separator),
        LC      => standalone_128_lc::<W, N>(bytes, radix, digit_separator),
        T       => standalone_128_t::<W, N>(bytes, radix, digit_separator),
        TC      => standalone_128_tc::<W, N>(bytes, radix, digit_separator),
        IL      => standalone_128_il::<W, N>(bytes, radix, digit_separator),
        ILC     => standalone_128_ilc::<W, N>(bytes, radix, digit_separator),
        IT      => standalone_128_it::<W, N>(bytes, radix, digit_separator),
        ITC     => standalone_128_itc::<W, N>(bytes, radix, digit_separator),
        LT      => standalone_128_lt::<W, N>(bytes, radix, digit_separator),
        LTC     => standalone_128_ltc::<W, N>(bytes, radix, digit_separator),
        ILT     => standalone_128_ilt::<W, N>(bytes, radix, digit_separator),
        ILTC    => standalone_128_iltc::<W, N>(bytes, radix, digit_separator),
        // No digit separator match.
        _       => standalone_128::<W, N>(bytes, radix)
    }?;

    // Check if we have any leading zeros.
    if format.no_integer_leading_zeros() {
        validate_no_leading_zeros(bytes, digit_separator, ptr)?;
    }

    Ok((value, ptr))
}}
