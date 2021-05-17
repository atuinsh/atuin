//! Mildly fast, generic, lexical integer-to-string conversion routines.

//  The following benchmarks were run on an "Intel(R) Core(TM) i7-6560U
//  CPU @ 2.20GHz" CPU, on Fedora 28, Linux kernel version 4.18.16-200
//  (x86-64), using the lexical formatter, `itoa::write()` or `x.to_string()`,
//  avoiding any inefficiencies in Rust string parsing for `format!(...)`
//  or `write!()` macros. The code was compiled with LTO and at an optimization
//  level of 3.
//
//  The benchmarks with `std` were compiled using "rustc 1.32.0
// (9fda7c223 2019-01-16".
//
//  The benchmark code may be found `benches/itoa.rs`.
//
//  # Benchmarks
//
//  | Type  |  lexical (ns/iter) | libcore (ns/iter)     | Relative Increase |
//  |:-----:|:------------------:|:---------------------:|:-----------------:|
//  | u8    | 122,329            | 413,025               | 3.38x             |
//  | u16   | 119,888            | 405,945               | 3.41x             |
//  | u32   | 121,150            | 423,174               | 3.49x             |
//  | u64   | 165,609            | 531,862               | 3.21x             |
//  | i8    | 151,478            | 458,374               | 3.03x             |
//  | i16   | 153,211            | 489,010               | 3.19x             |
//  | i32   | 149,433            | 517,710               | 3.46x             |
//  | i64   | 195,575            | 553,387               | 2.83x             |
//
//  # Raw Benchmarks
//
//  ```text
//  test itoa_i8_itoa     ... bench:     130,969 ns/iter (+/- 7,420)
//  test itoa_i8_lexical  ... bench:     151,478 ns/iter (+/- 7,510)
//  test itoa_i8_std      ... bench:     458,374 ns/iter (+/- 26,663)
//  test itoa_i16_itoa    ... bench:     143,344 ns/iter (+/- 9,495)
//  test itoa_i16_lexical ... bench:     153,211 ns/iter (+/- 7,365)
//  test itoa_i16_std     ... bench:     489,010 ns/iter (+/- 25,319)
//  test itoa_i32_itoa    ... bench:     176,494 ns/iter (+/- 9,596)
//  test itoa_i32_lexical ... bench:     149,433 ns/iter (+/- 5,803)
//  test itoa_i32_std     ... bench:     517,710 ns/iter (+/- 38,439)
//  test itoa_i64_itoa    ... bench:     205,055 ns/iter (+/- 12,436)
//  test itoa_i64_lexical ... bench:     195,575 ns/iter (+/- 8,007)
//  test itoa_i64_std     ... bench:     553,387 ns/iter (+/- 26,731)
//  test itoa_u8_itoa     ... bench:     112,529 ns/iter (+/- 4,514)
//  test itoa_u8_lexical  ... bench:     122,329 ns/iter (+/- 9,902)
//  test itoa_u8_std      ... bench:     413,025 ns/iter (+/- 30,262)
//  test itoa_u16_itoa    ... bench:      91,936 ns/iter (+/- 5,405)
//  test itoa_u16_lexical ... bench:     119,888 ns/iter (+/- 6,089)
//  test itoa_u16_std     ... bench:     405,945 ns/iter (+/- 24,104)
//  test itoa_u32_itoa    ... bench:     161,679 ns/iter (+/- 6,719)
//  test itoa_u32_lexical ... bench:     121,150 ns/iter (+/- 7,580)
//  test itoa_u32_std     ... bench:     423,174 ns/iter (+/- 21,801)
//  test itoa_u64_itoa    ... bench:     203,847 ns/iter (+/- 18,512)
//  test itoa_u64_lexical ... bench:     165,609 ns/iter (+/- 8,620)
//  test itoa_u64_std     ... bench:     531,862 ns/iter (+/- 31,223)
//  ```

// Code the generate the benchmark plot:
//  import numpy as np
//  import pandas as pd
//  import matplotlib.pyplot as plt
//  plt.style.use('ggplot')
//  lexical = np.array([122329, 119888, 121150, 165609, 151478, 153211, 149433, 195575]) / 1e6
//  itoa = np.array([112529, 91936, 161679, 203847, 130969, 143344, 176494, 205055]) / 1e6
//  rustcore = np.array([413025, 405945, 423174, 531862, 458374, 489010, 517710, 553387]) / 1e6
//  index = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"]
//  df = pd.DataFrame({'lexical': lexical, 'itoa': itoa, 'rustcore': rustcore}, index = index, columns=['lexical', 'itoa', 'rustcore'])
//  ax = df.plot.bar(rot=0, figsize=(16, 8), fontsize=14, color=['#E24A33', '#988ED5', '#348ABD'])
//  ax.set_ylabel("ms/iter")
//  ax.figure.tight_layout()
//  ax.legend(loc=2, prop={'size': 14})
//  plt.show()

use crate::util::*;

// Generic itoa algorithm.
macro_rules! generic_algorithm {
    ($value:ident, $radix:ident, $buffer:ident, $t:tt, $table:ident, $index:ident, $radix2:ident, $radix4:ident) => ({
        while $value >= $radix4 {
            let r = $value % $radix4;
            $value /= $radix4;
            let r1 = ($t::TWO * (r / $radix2)).as_usize();
            let r2 = ($t::TWO * (r % $radix2)).as_usize();

            // This is always safe, since the table is 2*radix^2, and
            // r1 and r2 must be in the range [0, 2*radix^2-1), since the maximum
            // value of r is `radix4-1`, which must have a div and r
            // in the range [0, radix^2-1).
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r2+1]));
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r2]));
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r1+1]));
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r1]));
        }

        // Decode 2 digits at a time.
        while $value >= $radix2 {
            let r = ($t::TWO * ($value % $radix2)).as_usize();
            $value /= $radix2;

            // This is always safe, since the table is 2*radix^2, and
            // r must be in the range [0, 2*radix^2-1).
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r+1]));
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r]));
        }

        // Decode last 2 digits.
        if $value < $radix {
            // This is always safe, since value < radix, so it must be < 36.
            // Digit must be <= 36.
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = digit_to_char($value));
            //*iter.next().unwrap() = digit_to_char(value);
        } else {
            let r = ($t::TWO * $value).as_usize();
            // This is always safe, since the table is 2*radix^2, and the value
            // must <= radix^2, so rem must be in the range [0, 2*radix^2-1).
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r+1]));
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = unchecked_index!($table[r]));
        }
    });
}

// Get lookup table for 2 digit radix conversions.
perftools_inline!{
#[cfg(feature = "radix")]
fn get_table(radix: u32) -> &'static [u8] {
    match radix {
        2   => &DIGIT_TO_BASE2_SQUARED,
        3   => &DIGIT_TO_BASE3_SQUARED,
        4   => &DIGIT_TO_BASE4_SQUARED,
        5   => &DIGIT_TO_BASE5_SQUARED,
        6   => &DIGIT_TO_BASE6_SQUARED,
        7   => &DIGIT_TO_BASE7_SQUARED,
        8   => &DIGIT_TO_BASE8_SQUARED,
        9   => &DIGIT_TO_BASE9_SQUARED,
        10  => &DIGIT_TO_BASE10_SQUARED,
        11  => &DIGIT_TO_BASE11_SQUARED,
        12  => &DIGIT_TO_BASE12_SQUARED,
        13  => &DIGIT_TO_BASE13_SQUARED,
        14  => &DIGIT_TO_BASE14_SQUARED,
        15  => &DIGIT_TO_BASE15_SQUARED,
        16  => &DIGIT_TO_BASE16_SQUARED,
        17  => &DIGIT_TO_BASE17_SQUARED,
        18  => &DIGIT_TO_BASE18_SQUARED,
        19  => &DIGIT_TO_BASE19_SQUARED,
        20  => &DIGIT_TO_BASE20_SQUARED,
        21  => &DIGIT_TO_BASE21_SQUARED,
        22  => &DIGIT_TO_BASE22_SQUARED,
        23  => &DIGIT_TO_BASE23_SQUARED,
        24  => &DIGIT_TO_BASE24_SQUARED,
        25  => &DIGIT_TO_BASE25_SQUARED,
        26  => &DIGIT_TO_BASE26_SQUARED,
        27  => &DIGIT_TO_BASE27_SQUARED,
        28  => &DIGIT_TO_BASE28_SQUARED,
        29  => &DIGIT_TO_BASE29_SQUARED,
        30  => &DIGIT_TO_BASE30_SQUARED,
        31  => &DIGIT_TO_BASE31_SQUARED,
        32  => &DIGIT_TO_BASE32_SQUARED,
        33  => &DIGIT_TO_BASE33_SQUARED,
        34  => &DIGIT_TO_BASE34_SQUARED,
        35  => &DIGIT_TO_BASE35_SQUARED,
        36  => &DIGIT_TO_BASE36_SQUARED,
        _   => unreachable!(),
    }
}}

// Get lookup table for 2 digit radix conversions.
perftools_inline!{
#[cfg(not(feature = "radix"))]
fn get_table(_: u32) -> &'static [u8] {
   &DIGIT_TO_BASE10_SQUARED
}}

// Optimized implementation for radix-N numbers.
// Precondition: `value` must be non-negative and mutable.
perftools_inline!{
#[allow(unused_unsafe)]
fn generic<T>(mut value: T, radix: u32, table: &[u8], buffer: &mut [u8])
    -> usize
    where T: UnsignedInteger
{
    // Both forms of unchecked indexing cannot overflow.
    // The table always has 2*radix^2 elements, so it must be a legal index.
    // The buffer is ensured to have at least MAX_DIGITS or MAX_DIGITS_BASE10
    // characters, which is the maximum number of digits an integer of
    // that size may write.

    // Use power-reduction to minimize the number of operations.
    // Idea taken from "3 Optimization Tips for C++".
    let radix: T = as_cast(radix);
    let radix2 = radix * radix;
    let radix4 = radix2 * radix2;

    // Decode 4-digits at a time
    let mut index = buffer.len();
    generic_algorithm!(value, radix, buffer, T, table, index, radix2, radix4);
    index
}}

// Optimized implementation for radix-N numbers.
// Precondition:
//  `value` must be non-negative and mutable.
//  Buffer must be 0-initialized.
perftools_inline!{
#[allow(unused_unsafe)]
fn generic_u128(value: u128, radix: u32, table: &[u8], buffer: &mut [u8])
    -> usize
{
    // Both forms of unchecked indexing cannot overflow.
    // The table always has 2*radix^2 elements, so it must be a legal index.
    // The buffer is ensured to have at least MAX_DIGITS or MAX_DIGITS_BASE10
    // characters, which is the maximum number of digits an integer of
    // that size may write.

    // Use power-reduction to minimize the number of operations.
    // Idea taken from "3 Optimization Tips for C++".
    let (divisor, digits_per_iter, d_cltz) = u128_divisor(radix);
    let radix: u64 = as_cast(radix);
    let radix2 = radix * radix;
    let radix4 = radix2 * radix2;

    // Decode 4-digits at a time.
    // To deal with internal 0 values or values with internal 0 digits set,
    // we store the starting index, and if not all digits are written,
    // we just skip down `digits` digits for the next value.
    let mut index = buffer.len();
    let mut start_index = index;
    let (value, mut low) = u128_divrem(value, divisor, d_cltz);
    generic_algorithm!(low, radix, buffer, u64, table, index, radix2, radix4);
    if value != 0 {
        start_index -= digits_per_iter;
        index = index.min(start_index);
        let (value, mut mid) = u128_divrem(value, divisor, d_cltz);
        generic_algorithm!(mid, radix, buffer, u64, table, index, radix2, radix4);

        if value != 0 {
            start_index -= digits_per_iter;
            index = index.min(start_index);
            let mut high = value as u64;
            generic_algorithm!(high, radix, buffer, u64, table, index, radix2, radix4);
        }
    }
    index
}}

pub(crate) trait Generic {
    // Export integer to string.
    fn generic(self, radix: u32, buffer: &mut [u8]) -> usize;
}

// Implement generic for type.
macro_rules! generic_impl {
    ($($t:ty)*) => ($(
        impl Generic for $t {
            perftools_inline_always!{
            fn generic(self, radix: u32, buffer: &mut [u8]) -> usize {
                let table = get_table(radix);
                generic(self, radix, table, buffer)
            }}
        }
    )*);
}

generic_impl! { u8 u16 u32 u64 usize }

impl Generic for u128 {
    perftools_inline_always!{
    fn generic(self, radix: u32, buffer: &mut [u8]) -> usize {
        let table = get_table(radix);
        generic_u128(self, radix, table, buffer)
    }}
}
