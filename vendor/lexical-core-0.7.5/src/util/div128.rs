//! Optimized division algorithms for u128.
//!
//! The code in this module is derived off of `dtolnay/itoa`
//! and Rust's compiler-builtins crate. This copies a specific
//! path of LLVM's `__udivmodti4` intrinsic, which does division/
//! modulus for u128 in a single step. Rust implements both division
//! and modulus in terms of this intrinsic, but calls the intrinsic
//! twice for subsequent division and modulus operations on the same
//! dividend/divisor, leading to significant performance overhead.
//!
//! This module calculates the optimal divisors for each radix,
//! and exports a general-purpose division algorithm for u128 where
//! the divisor can fit in a u64.
//!
//! This implementation is derived from dtolnay/itoa, which can be found here:
//!     https://github.com/dtolnay/itoa/blob/master/src/udiv128.rs
//!
//! This implementation is also derived from Rust's compiler-builtins crate,
//! which can be found here:
//!     https://github.com/rust-lang-nursery/compiler-builtins/blob/master/src/int/udiv.rs
//!
//! Licensing for this module may be under the MIT or Illinois license
//! (a BSD-like license), and may be found here:
//!     https://github.com/rust-lang-nursery/compiler-builtins/blob/master/LICENSE.TXT

// Get the divisor for optimized 128-bit division.
// Returns the divisor, the number of digits processed, and the
// number of leading zeros in the divisor.
//
// These values were calculated using the following script:
//
//  ```text
//  import math
//
//  u64_max = 2**64 - 1
//  u128_max = 2**128-1
//
//  def is_valid(x):
//      return (
//          x <= u64_max
//          and (u128_max / (x**2)) < x
//      )
//
//  def find_pow(radix):
//      start_pow = int(math.floor(math.log(u64_max, radix))) - 1
//      while is_valid(radix**start_pow):
//          start_pow += 1
//      return start_pow - 1
//
//  for radix in range(2, 37):
//      power = find_pow(radix)
//      print(radix, radix**power, power)
//  ```
#[cfg(feature = "radix")]
#[inline]
pub(crate) fn u128_divisor(radix: u32) -> (u64, usize, u32) {
    match radix {
        2  => (9223372036854775808, 63, 0),    // 2^63
        3  => (12157665459056928801, 40, 0),   // 3^40
        4  => (4611686018427387904, 31, 1),    // 4^31
        5  => (7450580596923828125, 27, 1),    // 5^27
        6  => (4738381338321616896, 24, 1),    // 6^24
        7  => (3909821048582988049, 22, 2),    // 7^22
        8  => (9223372036854775808, 21, 0),    // 8^21
        9  => (12157665459056928801, 20, 0),   // 9^20
        10 => (10000000000000000000, 19, 0),   // 10^19
        11 => (5559917313492231481, 18, 1),    // 11^18
        12 => (2218611106740436992, 17, 3),    // 12^17
        13 => (8650415919381337933, 17, 1),    // 13^17
        14 => (2177953337809371136, 16, 3),    // 14^16
        15 => (6568408355712890625, 16, 1),    // 15^16
        16 => (1152921504606846976, 15, 3),    // 16^15
        17 => (2862423051509815793, 15, 2),    // 17^15
        18 => (6746640616477458432, 15, 1),    // 18^15
        19 => (15181127029874798299, 15, 0),   // 19^15
        20 => (1638400000000000000, 14, 3),    // 20^14
        21 => (3243919932521508681, 14, 2),    // 21^14
        22 => (6221821273427820544, 14, 1),    // 22^14
        23 => (11592836324538749809, 14, 0),   // 23^14
        24 => (876488338465357824, 13, 4),     // 24^13
        25 => (1490116119384765625, 13, 3),    // 25^13
        26 => (2481152873203736576, 13, 2),    // 26^13
        27 => (4052555153018976267, 13, 2),    // 27^13
        28 => (6502111422497947648, 13, 1),    // 28^13
        29 => (10260628712958602189, 13, 0),   // 29^13
        30 => (15943230000000000000, 13, 0),   // 30^13
        31 => (787662783788549761, 12, 4),     // 31^12
        32 => (1152921504606846976, 12, 3),    // 32^12
        33 => (1667889514952984961, 12, 3),    // 33^12
        34 => (2386420683693101056, 12, 2),    // 34^12
        35 => (3379220508056640625, 12, 2),    // 35^12
        36 => (4738381338321616896, 12, 1),    // 36^12
        _  => unreachable!(),
    }
}

// Get the divisor for optimized 128-bit division.
// Returns the divisor, the number of digits processed, and the
// number of leading zeros in the divisor.
#[cfg(not(feature = "radix"))]
#[inline]
#[allow(dead_code)]
pub(crate) fn u128_divisor(_: u32) -> (u64, usize, u32) {
    (10000000000000000000, 19, 0)              // 10^19
}

// Optimized division/remainder algorithm for u128.
// This is because the codegen for u128 divrem is very inefficient in Rust,
// calling both `__udivmodti4` twice internally, rather than a single time.
#[inline]
pub(crate) fn u128_divrem(n: u128, d: u64, d_cltz: u32) -> (u128, u64) {
    // Ensure we have the correct number of leading zeros passed.
    debug_assert_eq!(d_cltz, d.leading_zeros());

    // Optimize if we can divide using u64 first.
    let high = (n >> 64) as u64;
    if high == 0 {
        let low = n as u64;
        return ((low / d) as u128, low % d);
    }

    // sr = 1 + u64::BITS + d.leading_zeros() - high.leading_zeros();
    let sr = 65 + d_cltz - high.leading_zeros();

    // 1 <= sr <= u64::BITS - 1
    let mut q: u128 = n << (128 - sr);
    let mut r: u128 = n >> sr;
    let mut carry: u64 = 0;

    // Don't use a range because they may generate references to memcpy in unoptimized code
    // Loop invariants:  r < d; carry is 0 or 1
    let mut i = 0;
    while i < sr {
        i += 1;

        // r:q = ((r:q) << 1) | carry
        r = (r << 1) | (q >> 127);
        q = (q << 1) | carry as u128;

        // carry = 0
        // if r >= d {
        //     r -= d;
        //     carry = 1;
        // }
        let s = (d as u128).wrapping_sub(r).wrapping_sub(1) as i128 >> 127;
        carry = (s & 1) as u64;
        r -= (d as u128) & s as u128;
    }

    ((q << 1) | carry as u128, r as u64)
}

// Divide by 1e19 for base10 algorithms.
#[cfg(feature = "table")]
pub(crate) fn u128_divrem_1e19(n: u128) -> (u128, u64) {
    u128_divrem(n, 10000000000000000000, 0)
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "std")]
    proptest! {
        #[test]
        fn u128_divrem_proptest(i in u128::min_value()..u128::max_value()) {
            let (d, _, d_cltz) = u128_divisor(10);
            let expected = (i / d as u128, (i % d as u128) as u64);
            let actual = u128_divrem(i, d, d_cltz);
            prop_assert_eq!(actual, expected);
        }
    }
}
