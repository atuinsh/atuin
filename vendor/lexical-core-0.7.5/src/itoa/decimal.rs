//! Fast lexical integer-to-string conversion routines for decimal strings.

//  The following algorithms aim to minimize the number of conditional
//  jumps required, by requiring at most 5 linear conditions before
//  jumping to a condition-less set of instructions. This allows high
//  performance formatting for integer sizes, and scales well for
//  both sequential values (primarily low number of digits) and uniform
//  values (primarily high numbers of digits), however, it also works
//  well even with branch misprediction (tested using a linear congruent
//  generator to choose between a sequential or uniform integer).
//
//  The performance is ~2-3x the performance of traditional integer
//  formatters (see, dtolnay/itoa, or the generic algorithm) for 32-bits
//  or less, highlighting the advantage of removing for loops with
//  minimal branches. It also scales well for 64 or more bit integers.

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
//  | u8    | 55,072             | 376,625               | 6.84x             |
//  | u16   | 51,219             | 385,722               | 7.53x             |
//  | u32   | 120,378            | 410,117               | 3.41x             |
//  | u64   | 187,850            | 489,783               | 2.60x             |
//  | u128  | 2,056,008          | 14,556,649            | 7.08x             |
//  | i8    | 81,346             | 414,715               | 5.10x             |
//  | i16   | 102,664            | 447,581               | 5.50x             |
//  | i32   | 149,340            | 475,189               | 3.18x             |
//  | i64   | 230,283            | 527,589               | 2.29x             |
//  | i128  | 2,052,915          | 14,600,861            | 7.11x             |
//
//  # Raw Benchmarks
//
//  ```text
//  test itoa_i8_itoa                    ... bench:     126,392 ns/iter (+/- 5,778)
//  test itoa_i8_lexical                 ... bench:      81,346 ns/iter (+/- 1,476)
//  test itoa_i8_std                     ... bench:     414,715 ns/iter (+/- 5,695)
//  test itoa_i16_itoa                   ... bench:     142,794 ns/iter (+/- 8,103)
//  test itoa_i16_lexical                ... bench:     102,664 ns/iter (+/- 2,735)
//  test itoa_i16_std                    ... bench:     447,581 ns/iter (+/- 47,814)
//  test itoa_i32_itoa                   ... bench:     173,478 ns/iter (+/- 7,305)
//  test itoa_i32_lexical                ... bench:     149,340 ns/iter (+/- 9,998)
//  test itoa_i32_std                    ... bench:     475,189 ns/iter (+/- 32,131)
//  test itoa_i64_itoa                   ... bench:     198,176 ns/iter (+/- 16,321)
//  test itoa_i64_lexical                ... bench:     230,283 ns/iter (+/- 5,156)
//  test itoa_i64_std                    ... bench:     527,589 ns/iter (+/- 9,557)
//  test itoa_i128_itoa                  ... bench:   2,047,257 ns/iter (+/- 73,000)
//  test itoa_i128_lexical               ... bench:   2,052,915 ns/iter (+/- 74,725)
//  test itoa_i128_std                   ... bench:  14,600,861 ns/iter (+/- 271,447)
//  test itoa_u8_heterogeneous_itoa      ... bench:     292,486 ns/iter (+/- 9,220)
//  test itoa_u8_heterogeneous_lexical   ... bench:     206,873 ns/iter (+/- 2,046)
//  test itoa_u8_heterogeneous_std       ... bench:     750,418 ns/iter (+/- 15,635)
//  test itoa_u8_itoa                    ... bench:     105,066 ns/iter (+/- 2,855)
//  test itoa_u8_lexical                 ... bench:      55,072 ns/iter (+/- 1,549)
//  test itoa_u8_simple_itoa             ... bench:      69,004 ns/iter (+/- 1,619)
//  test itoa_u8_simple_lexical          ... bench:      28,524 ns/iter (+/- 1,577)
//  test itoa_u8_simple_std              ... bench:     317,812 ns/iter (+/- 14,782)
//  test itoa_u8_std                     ... bench:     376,625 ns/iter (+/- 10,076)
//  test itoa_u16_heterogeneous_itoa     ... bench:     286,189 ns/iter (+/- 16,636)
//  test itoa_u16_heterogeneous_lexical  ... bench:     214,915 ns/iter (+/- 7,595)
//  test itoa_u16_heterogeneous_std      ... bench:     797,362 ns/iter (+/- 39,345)
//  test itoa_u16_itoa                   ... bench:      90,015 ns/iter (+/- 1,597)
//  test itoa_u16_lexical                ... bench:      51,219 ns/iter (+/- 5,323)
//  test itoa_u16_simple_itoa            ... bench:      92,558 ns/iter (+/- 2,638)
//  test itoa_u16_simple_lexical         ... bench:      42,701 ns/iter (+/- 3,799)
//  test itoa_u16_simple_std             ... bench:     363,527 ns/iter (+/- 20,206)
//  test itoa_u16_std                    ... bench:     385,722 ns/iter (+/- 14,945)
//  test itoa_u32_heterogeneous_itoa     ... bench:     319,279 ns/iter (+/- 8,077)
//  test itoa_u32_heterogeneous_lexical  ... bench:     262,614 ns/iter (+/- 10,914)
//  test itoa_u32_heterogeneous_std      ... bench:     830,270 ns/iter (+/- 38,468)
//  test itoa_u32_itoa                   ... bench:     114,494 ns/iter (+/- 2,552)
//  test itoa_u32_lexical                ... bench:     120,378 ns/iter (+/- 4,930)
//  test itoa_u32_simple_itoa            ... bench:      89,981 ns/iter (+/- 4,136)
//  test itoa_u32_simple_lexical         ... bench:      42,902 ns/iter (+/- 2,875)
//  test itoa_u32_simple_std             ... bench:     366,203 ns/iter (+/- 14,009)
//  test itoa_u32_std                    ... bench:     410,117 ns/iter (+/- 15,781)
//  test itoa_u64_heterogeneous_itoa     ... bench:     399,933 ns/iter (+/- 9,702)
//  test itoa_u64_heterogeneous_lexical  ... bench:     348,971 ns/iter (+/- 5,901)
//  test itoa_u64_heterogeneous_std      ... bench:     928,328 ns/iter (+/- 27,413)
//  test itoa_u64_itoa                   ... bench:     205,365 ns/iter (+/- 20,925)
//  test itoa_u64_lexical                ... bench:     187,850 ns/iter (+/- 5,501)
//  test itoa_u64_simple_itoa            ... bench:      91,360 ns/iter (+/- 3,417)
//  test itoa_u64_simple_lexical         ... bench:      43,714 ns/iter (+/- 771)
//  test itoa_u64_simple_std             ... bench:     373,320 ns/iter (+/- 6,293)
//  test itoa_u64_std                    ... bench:     489,783 ns/iter (+/- 27,719)
//  test itoa_u128_heterogeneous_itoa    ... bench:   2,313,010 ns/iter (+/- 56,841)
//  test itoa_u128_heterogeneous_lexical ... bench:   2,219,170 ns/iter (+/- 61,161)
//  test itoa_u128_heterogeneous_std     ... bench:  15,076,380 ns/iter (+/- 206,851)
//  test itoa_u128_itoa                  ... bench:   2,084,005 ns/iter (+/- 55,530)
//  test itoa_u128_lexical               ... bench:   2,056,008 ns/iter (+/- 76,115)
//  test itoa_u128_simple_itoa           ... bench:     112,675 ns/iter (+/- 6,788)
//  test itoa_u128_simple_lexical        ... bench:      59,320 ns/iter (+/- 2,386)
//  test itoa_u128_simple_std            ... bench:     365,819 ns/iter (+/- 17,862)
//  test itoa_u128_std                   ... bench:  14,556,649 ns/iter (+/- 238,244)
//  ```

// Code the generate the benchmark plot:
//  import numpy as np
//  import pandas as pd
//  import matplotlib.pyplot as plt
//  plt.style.use('ggplot')
//  lexical = np.array([55072, 51219, 120378, 187850, 2056008, 81346, 102664, 149340, 230283, 2052915]) / 1e3
//  itoa = np.array([105066, 90015, 114494, 205365, 2084005, 126392, 142794, 173478, 198176, 2047257]) / 1e3
//  rustcore = np.array([376625, 385722, 410117, 489783, 14556649, 414715, 447581, 475189, 527589, 14600861]) / 1e3
//  index = ["u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128"]
//  df = pd.DataFrame({'lexical': lexical, 'itoa': itoa, 'rustcore': rustcore}, index = index, columns=['lexical', 'itoa', 'rustcore'])
//  ax = df.plot.bar(rot=0, figsize=(16, 8), fontsize=14, color=['#E24A33', '#988ED5', '#348ABD'])
//  ax.set_ylabel("ms/iter")
//  ax.set_yscale('log')
//  ax.figure.tight_layout()
//  ax.legend(loc=2, prop={'size': 14})
//  plt.show()

use crate::util::*;

// Lookup table for optimized base10 itoa.
const TABLE: &[u8] = &DIGIT_TO_BASE10_SQUARED;

// DIGIT COUNT
// -----------

// Hyper-optimized integer formatters using bit-twiddling tricks for
// base10. These are meant to be correct, however, they use bit-twiddling
// tricks so they may not be very legible.

// Calculate the number of leading 0s.
macro_rules! cltz {
    ($value:ident) => {
        $value.leading_zeros().as_usize()
    };
}

// Calculate the offset where the digits were first written.
macro_rules! calculate_offset {
    ($value:ident, $digits:ident, $max_digits:expr, $size:expr) => ({
        // Get the log2 of the value to estimate the log10 quickly.
        // log2(0) is undefined, always ensure 1 bit is set.
        let value = $value | 1;
        let log2 = $size - cltz!(value);

        // Estimate log10(value) to calculate number of digits.
        // Put in safe guards so we always have at least 1 digit.
        // Our magic numbers are:
        //  1233 / 2^12 == log10(2)
        // These magic numbers are valid for any value <= 2**18,
        // which encompasses all offsets (<= 40).
        let digits = (log2 * 1233) >> 12;
        let mut offset = $max_digits - digits - 1;
        debug_assert!(offset < $digits.len());
        if digits != 0 && unchecked_index!($digits[offset]) == b'0' {
            offset += 1;
        }

        offset
    });
}

// INDEXING
// --------

// Convert sequential values to index.
macro_rules! sequential_index {
    ($v0:ident, $v1:ident) => (($v0 * 2 - $v1 * 200).as_usize());
}

// Convert singular value to index.
macro_rules! last_index {
    ($value:ident) => ((2 * $value).as_usize());
}

// WRITE
// -----

// Write N digits to buffer.

// Write 1 digit to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_1(value: u32, buffer: &mut [u8]) {
    unchecked_index_mut!(buffer[0] = digit_to_char(value));
}}

// Write 2 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_2(value: u32, buffer: &mut [u8]) {
    let i_0 = last_index!(value);
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_0+0]));
}}

// Write 3 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_3(value: u32, buffer: &mut [u8]) {
    let v_0 = value;
    let v_1 = v_0 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = last_index!(v_1);
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_1+1]));
}}

// Write 4 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_4(value: u32, buffer: &mut [u8]) {
    let v_0 = value;
    let v_1 = v_0 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = last_index!(v_1);
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_1+0]));
}}

// Write 5 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_5(value: u32, buffer: &mut [u8]) {
    let v_0 = value;
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = last_index!(v_2);
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_2+1]));
}}

// Write 10 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_10(value: u32, buffer: &mut [u8]) {
    let t0 = value / 100000000;
    let v_0 = value.wrapping_sub(t0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t0;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = last_index!(v_4);
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_4+0]));
}}

// Write 15 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_15(value: u64, buffer: &mut [u8]) {
    let t_0 = (value / 100000000).as_u32();
    let v_0 = value.as_u32().wrapping_sub(t_0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t_0;
    let v_5 = v_4 / 100;
    let v_6 = v_5 / 100;
    let v_7 = v_6 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = sequential_index!(v_4, v_5);
    let i_5 = sequential_index!(v_5, v_6);
    let i_6 = sequential_index!(v_6, v_7);
    let i_7 = last_index!(v_7);
    unchecked_index_mut!(buffer[14] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[13] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[12] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[11] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[10] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_4+0]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_5+1]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_5+0]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_6+1]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_6+0]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_7+1]));
}}

// Write 19 digits to buffer (used internally for the u128 writers).
perftools_inline!{
#[allow(unused_unsafe)]
fn write_19(value: u64, buffer: &mut [u8]) {
    let t_0 = (value / 100000000).as_u32();
    let t_1 = (value / 10000000000000000).as_u32();
    let v_0 = value.as_u32().wrapping_sub(t_0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t_0.wrapping_sub(t_1.wrapping_mul(100000000));
    let v_5 = v_4 / 100;
    let v_6 = v_5 / 100;
    let v_7 = v_6 / 100;
    let v_8 = t_1;
    let v_9 = v_8 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = sequential_index!(v_4, v_5);
    let i_5 = sequential_index!(v_5, v_6);
    let i_6 = sequential_index!(v_6, v_7);
    let i_7 = last_index!(v_7);
    let i_8 = sequential_index!(v_8, v_9);
    let i_9 = last_index!(v_9);
    unchecked_index_mut!(buffer[18] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[17] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[16] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[15] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[14] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[13] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[12] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[11] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[10] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_4+0]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_5+1]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_5+0]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_6+1]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_6+0]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_7+1]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_7+0]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_8+1]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_8+0]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_9+1]));
}}

// Write 20 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_20(value: u64, buffer: &mut [u8]) {
    let t_0 = (value / 100000000).as_u32();
    let t_1 = (value / 10000000000000000).as_u32();
    let v_0 = value.as_u32().wrapping_sub(t_0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t_0.wrapping_sub(t_1.wrapping_mul(100000000));
    let v_5 = v_4 / 100;
    let v_6 = v_5 / 100;
    let v_7 = v_6 / 100;
    let v_8 = t_1;
    let v_9 = v_8 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = sequential_index!(v_4, v_5);
    let i_5 = sequential_index!(v_5, v_6);
    let i_6 = sequential_index!(v_6, v_7);
    let i_7 = last_index!(v_7);
    let i_8 = sequential_index!(v_8, v_9);
    let i_9 = last_index!(v_9);
    unchecked_index_mut!(buffer[19] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[18] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[17] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[16] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[15] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[14] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[13] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[12] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[11] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[10] = unchecked_index!(TABLE[i_4+0]));
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_5+1]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_5+0]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_6+1]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_6+0]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_7+1]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_7+0]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_8+1]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_8+0]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_9+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_9+0]));
}}

// Write 25 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_25(value: u128, buffer: &mut [u8]) {
    // Split value into high 6 and low 19.
    let (high, low) = u128_divrem_1e19(value);

    // Write low 19 to the end of the buffer.
    write_19(low, &mut unchecked_index_mut!(buffer[6..]));

    // Write high 6 to the front of the buffer.
    let value = high.as_u64();
    let v_0 = value.as_u32();
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = last_index!(v_2);
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_2+0]));
}}

// Write 29 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_29(value: u128, buffer: &mut [u8]) {
    // Split value into high 10 and low 19.
    let (high, low) = u128_divrem_1e19(value);

    // Write low 19 to the end of the buffer.
    write_19(low, &mut unchecked_index_mut!(buffer[10..]));

    // Write high 10 to the front of the buffer.
    let value = high.as_u64();
    let t_0 = (value / 100000000).as_u32();
    let v_0 = value.as_u32().wrapping_sub(t_0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t_0;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = last_index!(v_4);
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_4+0]));
}}

// Write 34 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_34(value: u128, buffer: &mut [u8]) {
    // Split value into high 15 and low 19.
    let (high, low) = u128_divrem_1e19(value);

    // Write low 19 to the end of the buffer.
    write_19(low, &mut unchecked_index_mut!(buffer[15..]));

    // Write high 15 to the front of the buffer.
    let value = high.as_u64();
    let t_0 = (value / 100000000).as_u32();
    let v_0 = value.as_u32().wrapping_sub(t_0.wrapping_mul(100000000));
    let v_1 = v_0 / 100;
    let v_2 = v_1 / 100;
    let v_3 = v_2 / 100;
    let v_4 = t_0;
    let v_5 = v_4 / 100;
    let v_6 = v_5 / 100;
    let v_7 = v_6 / 100;
    let i_0 = sequential_index!(v_0, v_1);
    let i_1 = sequential_index!(v_1, v_2);
    let i_2 = sequential_index!(v_2, v_3);
    let i_3 = last_index!(v_3);
    let i_4 = sequential_index!(v_4, v_5);
    let i_5 = sequential_index!(v_5, v_6);
    let i_6 = sequential_index!(v_6, v_7);
    let i_7 = last_index!(v_7);
    unchecked_index_mut!(buffer[14] = unchecked_index!(TABLE[i_0+1]));
    unchecked_index_mut!(buffer[13] = unchecked_index!(TABLE[i_0+0]));
    unchecked_index_mut!(buffer[12] = unchecked_index!(TABLE[i_1+1]));
    unchecked_index_mut!(buffer[11] = unchecked_index!(TABLE[i_1+0]));
    unchecked_index_mut!(buffer[10] = unchecked_index!(TABLE[i_2+1]));
    unchecked_index_mut!(buffer[9] = unchecked_index!(TABLE[i_2+0]));
    unchecked_index_mut!(buffer[8] = unchecked_index!(TABLE[i_3+1]));
    unchecked_index_mut!(buffer[7] = unchecked_index!(TABLE[i_3+0]));
    unchecked_index_mut!(buffer[6] = unchecked_index!(TABLE[i_4+1]));
    unchecked_index_mut!(buffer[5] = unchecked_index!(TABLE[i_4+0]));
    unchecked_index_mut!(buffer[4] = unchecked_index!(TABLE[i_5+1]));
    unchecked_index_mut!(buffer[3] = unchecked_index!(TABLE[i_5+0]));
    unchecked_index_mut!(buffer[2] = unchecked_index!(TABLE[i_6+1]));
    unchecked_index_mut!(buffer[1] = unchecked_index!(TABLE[i_6+0]));
    unchecked_index_mut!(buffer[0] = unchecked_index!(TABLE[i_7+1]));
}}

// Write 39 digits to buffer.
perftools_inline!{
#[allow(unused_unsafe)]
fn write_39(value: u128, buffer: &mut [u8]) {
    // Split value into high 20 and low 19.
    let (high, low) = u128_divrem_1e19(value);

    // Write low 19 to the end of the buffer.
    write_19(low, &mut unchecked_index_mut!(buffer[20..]));

    // Split the value into the high 1 and mid 19.
    let (high, mid) = u128_divrem_1e19(high);

    // Write mid 19 to the middle of the buffer.
    write_19(mid, &mut unchecked_index_mut!(buffer[1..]));

    // Write high 1 to the front of the buffer.
    unchecked_index_mut!(buffer[0] = digit_to_char(high));
}}

// WRITE RAMGE
// -----------

// Write range of digits to buffer, optionally using a temporary buffer
// and copying the digits over.

// Write 1-3 digits (from a u8 value).
perftools_inline!{
fn write_1_3(value: u32, buffer: &mut [u8]) -> usize {
    if value < 10 {
        write_1(value, buffer);
        1
    } else if value < 100 {
        write_2(value, buffer);
        2
    } else {
        write_3(value, buffer);
        3
    }
}}

// Write 1-3 digits (from a u16 value).
perftools_inline!{
fn write_1_5(value: u32, buffer: &mut [u8]) -> usize {
    if value < 10 {
        write_1(value, buffer);
        1
    } else if value < 100 {
        write_2(value, buffer);
        2
    } else if value < 1000 {
        write_3(value, buffer);
        3
    } else if value < 10000 {
        write_4(value, buffer);
        4
    } else {
        write_5(value, buffer);
        5
    }
}}

// Write 5-10 digits (from a u32 value).
perftools_inline!{
fn write_5_10(value: u32, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 16] = [b'0'; 16];
    let digits = &mut tmp_buf[..10];
    write_10(value, digits);
    let offset = calculate_offset!(value, digits, 10, 32);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 10-15 digits (from a u64 value).
perftools_inline!{
fn write_10_15(value: u64, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 32] = [b'0'; 32];
    let digits = &mut tmp_buf[..15];
    write_15(value, digits);
    let offset = calculate_offset!(value, digits, 15, 64);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 15-20 digits (from a u64 value).
perftools_inline!{
fn write_15_20(value: u64, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 32] = [b'0'; 32];
    let digits = &mut tmp_buf[..20];
    write_20(value, digits);
    let offset = calculate_offset!(value, digits, 20, 64);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 20-25 digits (from a u64 value).
perftools_inline!{
fn write_20_25(value: u128, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 64] = [b'0'; 64];
    let digits = &mut tmp_buf[..25];
    write_25(value, digits);
    let offset = calculate_offset!(value, digits, 25, 128);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 25-29 digits (from a u64 value).
perftools_inline!{
fn write_25_29(value: u128, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 64] = [b'0'; 64];
    let digits = &mut tmp_buf[..29];
    write_29(value, digits);
    let offset = calculate_offset!(value, digits, 29, 128);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 29-34 digits (from a u64 value).
perftools_inline!{
fn write_29_34(value: u128, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 64] = [b'0'; 64];
    let digits = &mut tmp_buf[..34];
    write_34(value, digits);
    let offset = calculate_offset!(value, digits, 34, 128);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// Write 34-39 digits (from a u64 value).
perftools_inline!{
fn write_34_39(value: u128, buffer: &mut [u8]) -> usize {
    // Use a temporary buffer so we only need a single code path.
    let mut tmp_buf: [u8; 64] = [b'0'; 64];
    let digits = &mut tmp_buf[..39];
    write_39(value, digits);
    let offset = calculate_offset!(value, digits, 39, 128);
    copy_to_dst(buffer, &unchecked_index!(digits[offset..]))
}}

// FORMATTERS
// ----------

// Each flow-path should have no more than 5 comparisons, or
// else we're poorly optimizing our code.
// Use the number of leading zeros to minimize the number
// of jumps we have possible.

// Internal integer formatter for u8.
perftools_inline!{
fn u8toa(value: u8, buffer: &mut [u8]) -> usize {
    write_1_3(value.as_u32(), buffer)
}}

// Internal integer formatter for u16.
perftools_inline!{
fn u16toa(value: u16, buffer: &mut [u8]) -> usize {
    write_1_5(value.as_u32(), buffer)
}}

// Internal integer formatter for u32.
perftools_inline!{
fn u32toa(value: u32, buffer: &mut [u8]) -> usize {
    if value >> 16 == 0 {
        // [0, 2^16 - 1]
        write_1_5(value, buffer)
    } else {
        // [2^16, 2^32 - 1]
        write_5_10(value, buffer)
    }
}}

// Internal integer formatter for u64.
perftools_inline!{
fn u64toa(value: u64, buffer: &mut [u8]) -> usize {
    if value >> 16 == 0 {
        // [0, 2^16 - 1]
        write_1_5(value.as_u32(), buffer)
    } else if value >> 32 == 0 {
        // [2^16, 2^32 - 1]
        write_5_10(value.as_u32(), buffer)
    } else if value >> 48 == 0 {
        // [2^32, 2^48 - 1]
        write_10_15(value, buffer)
    } else {
        // [2^48, 2^64 - 1]
        write_15_20(value, buffer)
    }
}}

// Internal integer formatter for u128.
perftools_inline!{
fn u128toa(value: u128, buffer: &mut [u8]) -> usize {
    if value >> 16 == 0 {
        // [0, 2^16 - 1]
        write_1_5(value.as_u32(), buffer)
    } else if value >> 32 == 0 {
        // [2^16, 2^32 - 1]
        write_5_10(value.as_u32(), buffer)
    } else if value >> 48 == 0 {
        // [2^32, 2^48 - 1]
        write_10_15(value.as_u64(), buffer)
    } else if value >> 64 == 0 {
        // [2^48, 2^64 - 1]
        write_15_20(value.as_u64(), buffer)
    } else if value >> 80 == 0 {
        // [2^64, 2^80 - 1]
        write_20_25(value, buffer)
    } else if value >> 96 == 0 {
        // [2^80, 2^96 - 1]
        write_25_29(value, buffer)
    } else if value >> 112 == 0 {
        // [2^96, 2^112 - 1]
        write_29_34(value, buffer)
    } else {
        // [2^112, 2^128 - 1]
        write_34_39(value, buffer)
    }
}}

cfg_if! {
if #[cfg(target_pointer_width = "16")] {
    perftools_inline!{
    fn usizetoa(value: usize, buffer: &mut [u8]) -> usize {
        u16toa(value.as_u16(), buffer)
    }}
} else if #[cfg(target_pointer_width = "32")] {
    perftools_inline!{
    fn usizetoa(value: usize, buffer: &mut [u8]) -> usize {
        u32toa(value.as_u32(), buffer)
    }}
} else if #[cfg(target_pointer_width = "64")] {
    perftools_inline!{
    fn usizetoa(value: usize, buffer: &mut [u8]) -> usize {
        u64toa(value.as_u64(), buffer)
    }}
}} // cfg_if

// TRAIT
// -----

pub(crate) trait Decimal {
    // Export integer to string.
    fn decimal(self, buffer: &mut [u8]) -> usize;
}

// Implement decimal for type.
macro_rules! decimal_impl {
    ($t:ty, $cb:ident) => (
        impl Decimal for $t {
            perftools_inline_always!{
            fn decimal(self, buffer: &mut [u8]) -> usize {
                $cb(self, buffer)
            }}
        }
    );
}

decimal_impl!(u8, u8toa);
decimal_impl!(u16, u16toa);
decimal_impl!(u32, u32toa);
decimal_impl!(u64, u64toa);
decimal_impl!(u128, u128toa);
decimal_impl!(usize, usizetoa);
