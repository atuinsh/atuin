//! Low-level API generator.
//!
//! Uses either the optimized decimal algorithm, the optimized generic
//! algorithm, or the naive algorithm.

use crate::util::*;

/// Select the back-end.
#[cfg(feature = "table")]
use super::decimal::Decimal;

#[cfg(all(feature = "table", feature = "radix"))]
use super::generic::Generic;

#[cfg(not(feature = "table"))]
use super::naive::Naive;

// HELPERS

// Wrapper to facilitate calling a backend that writes iteratively to
// the end of the buffer.
#[cfg(any(not(feature = "table"), feature = "radix"))]
macro_rules! write_backwards {
    ($value:ident, $radix:expr, $buffer:ident, $t:tt, $cb:ident) => ({
        // Create a temporary buffer, and copy into it.
        // Way faster than reversing a buffer in-place.
        // Need to ensure the buffer size is adequate for any radix, but
        // small for the optimized decimal formatters.
        debug_assert_radix!($radix);
        let mut buffer: [u8; BUFFER_SIZE] = [b'0'; BUFFER_SIZE];
        let digits;
        if cfg!(not(feature = "radix")) || $radix == 10 {
            digits = &mut buffer[..$t::FORMATTED_SIZE_DECIMAL];
        } else {
            digits = &mut buffer[..$t::FORMATTED_SIZE];
        }

        // Write backwards to buffer and copy output to slice.
        let offset = $value.$cb($radix, digits);
        debug_assert!(offset <= digits.len());
        copy_to_dst($buffer, &unchecked_index!(digits[offset..]))
    });
}

#[cfg(all(feature = "table", not(feature = "radix")))]
pub(crate) trait Itoa: Decimal + UnsignedInteger
{}

#[cfg(all(feature = "table", feature = "radix"))]
pub(crate) trait Itoa: Decimal + Generic + UnsignedInteger
{}

#[cfg(not(feature = "table"))]
pub(crate) trait Itoa: Naive + UnsignedInteger
{}

macro_rules! itoa_impl {
    ($($t:ty)*) => ($(
        impl Itoa for $t {}
    )*)
}

itoa_impl! { u8 u16 u32 u64 u128 usize }

// FORWARD

// Forward itoa arguments to an optimized backend.
//  Preconditions: `value` must be non-negative and unsigned.
perftools_inline!{
#[cfg(all(feature = "table", not(feature = "radix")))]
pub(crate) fn itoa_positive<T>(value: T, _: u32, buffer: &mut [u8])
    -> usize
    where T: Itoa
{
    value.decimal(buffer)
}}

// Forward itoa arguments to an optimized backend.
//  Preconditions: `value` must be non-negative and unsigned.
perftools_inline!{
#[cfg(all(feature = "table", feature = "radix"))]
pub(crate) fn itoa_positive<T>(value: T, radix: u32, buffer: &mut [u8])
    -> usize
    where T: Itoa
{
    if radix == 10 {
        value.decimal(buffer)
    } else {
        write_backwards!(value, radix, buffer, T, generic)
    }
}}

// Forward itoa arguments to a naive backend.
//  Preconditions: `value` must be non-negative and unsigned.
perftools_inline!{
#[cfg(not(feature = "table"))]
pub(crate) fn itoa_positive<T>(value: T, radix: u32, buffer: &mut [u8])
    -> usize
    where T: Itoa
{
    write_backwards!(value, radix, buffer, T, naive)
}}

// TO LEXICAL

// Callback for unsigned integer formatter.
perftools_inline!{
fn unsigned<Narrow, Wide>(value: Narrow, radix: u32, buffer: &mut [u8])
    -> usize
    where Narrow: UnsignedInteger,
          Wide: Itoa
{
    let value: Wide = as_cast(value);
    itoa_positive(value, radix, buffer)
}}

macro_rules! unsigned_to_lexical {
    ($narrow:ty, $wide:ty) => (
        to_lexical!(unsigned::<$narrow, $wide>, $narrow);
    );
}

unsigned_to_lexical!(u8, u32);
unsigned_to_lexical!(u16, u32);
unsigned_to_lexical!(u32, u32);
unsigned_to_lexical!(u64, u64);
unsigned_to_lexical!(u128, u128);

#[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
unsigned_to_lexical!(usize, u32);

#[cfg(target_pointer_width = "64")]
unsigned_to_lexical!(usize, u64);

// Callback for signed integer formatter.
perftools_inline!{
fn signed<Narrow, Wide, Unsigned>(value: Narrow, radix: u32, buffer: &mut [u8])
    -> usize
    where Narrow: SignedInteger,
          Wide: SignedInteger,
          Unsigned: Itoa
{
    if value < Narrow::ZERO {
        unchecked_index_mut!(buffer[0] = b'-');
        let value: Wide = as_cast(value);
        let value: Unsigned = as_cast(value.wrapping_neg());
        itoa_positive(value, radix, &mut unchecked_index_mut!(buffer[1..])) + 1
    } else {
        let value: Unsigned = as_cast(value);
        itoa_positive(value, radix, buffer)
    }
}}

macro_rules! signed_to_lexical {
    ($narrow:ty, $wide:ty, $unsigned:ty) => (
        to_lexical!(signed::<$narrow, $wide, $unsigned>, $narrow);
    );
}

signed_to_lexical!(i8, i32, u32);
signed_to_lexical!(i16, i32, u32);
signed_to_lexical!(i32, i32, u32);
signed_to_lexical!(i64, i64, u64);
signed_to_lexical!(i128, i128, u128);

#[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
signed_to_lexical!(isize, i32, u32);

#[cfg(target_pointer_width = "64")]
signed_to_lexical!(isize, i64, u64);

// TESTS
// -----

#[cfg(test)]
mod tests {
    // Shouldn't need to include atoi, should be fine with ToLexical in scope.
    use crate::util::*;
    use crate::util::test::*;

    // GENERIC

    #[test]
    fn u8_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0u8.to_lexical(&mut buffer));
        assert_eq!(b"1", 1u8.to_lexical(&mut buffer));
        assert_eq!(b"5", 5u8.to_lexical(&mut buffer));
        assert_eq!(b"127", 127u8.to_lexical(&mut buffer));
        assert_eq!(b"128", 128u8.to_lexical(&mut buffer));
        assert_eq!(b"255", 255u8.to_lexical(&mut buffer));
        assert_eq!(b"255", (-1i8 as u8).to_lexical(&mut buffer));
    }

    #[test]
    fn i8_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0i8.to_lexical(&mut buffer));
        assert_eq!(b"1", 1i8.to_lexical(&mut buffer));
        assert_eq!(b"5", 5i8.to_lexical(&mut buffer));
        assert_eq!(b"127", 127i8.to_lexical(&mut buffer));
        assert_eq!(b"-128", (128u8 as i8).to_lexical(&mut buffer));
        assert_eq!(b"-1", (255u8 as i8).to_lexical(&mut buffer));
        assert_eq!(b"-1", (-1i8).to_lexical(&mut buffer));
    }

    #[test]
    fn u16_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0u16.to_lexical(&mut buffer));
        assert_eq!(b"1", 1u16.to_lexical(&mut buffer));
        assert_eq!(b"5", 5u16.to_lexical(&mut buffer));
        assert_eq!(b"32767", 32767u16.to_lexical(&mut buffer));
        assert_eq!(b"32768", 32768u16.to_lexical(&mut buffer));
        assert_eq!(b"65535", 65535u16.to_lexical(&mut buffer));
        assert_eq!(b"65535", (-1i16 as u16).to_lexical(&mut buffer));
    }

    #[test]
    fn i16_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0i16.to_lexical(&mut buffer));
        assert_eq!(b"1", 1i16.to_lexical(&mut buffer));
        assert_eq!(b"5", 5i16.to_lexical(&mut buffer));
        assert_eq!(b"32767", 32767i16.to_lexical(&mut buffer));
        assert_eq!(b"-32768", (32768u16 as i16).to_lexical(&mut buffer));
        assert_eq!(b"-1", (65535u16 as i16).to_lexical(&mut buffer));
        assert_eq!(b"-1", (-1i16).to_lexical(&mut buffer));
    }

    #[test]
    fn u32_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0u32.to_lexical(&mut buffer));
        assert_eq!(b"1", 1u32.to_lexical(&mut buffer));
        assert_eq!(b"5", 5u32.to_lexical(&mut buffer));
        assert_eq!(b"2147483647", 2147483647u32.to_lexical(&mut buffer));
        assert_eq!(b"2147483648", 2147483648u32.to_lexical(&mut buffer));
        assert_eq!(b"4294967295", 4294967295u32.to_lexical(&mut buffer));
        assert_eq!(b"4294967295", (-1i32 as u32).to_lexical(&mut buffer));
    }

    #[test]
    fn i32_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0i32.to_lexical(&mut buffer));
        assert_eq!(b"1", 1i32.to_lexical(&mut buffer));
        assert_eq!(b"5", 5i32.to_lexical(&mut buffer));
        assert_eq!(b"2147483647", 2147483647i32.to_lexical(&mut buffer));
        assert_eq!(b"-2147483648", (2147483648u32 as i32).to_lexical(&mut buffer));
        assert_eq!(b"-1", (4294967295u32 as i32).to_lexical(&mut buffer));
        assert_eq!(b"-1", (-1i32).to_lexical(&mut buffer));
    }

    #[test]
    fn u64_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0u64.to_lexical(&mut buffer));
        assert_eq!(b"1", 1u64.to_lexical(&mut buffer));
        assert_eq!(b"5", 5u64.to_lexical(&mut buffer));
        assert_eq!(b"9223372036854775807", 9223372036854775807u64.to_lexical(&mut buffer));
        assert_eq!(b"9223372036854775808", 9223372036854775808u64.to_lexical(&mut buffer));
        assert_eq!(b"18446744073709551615", 18446744073709551615u64.to_lexical(&mut buffer));
        assert_eq!(b"18446744073709551615", (-1i64 as u64).to_lexical(&mut buffer));
    }

    #[test]
    fn i64_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0i64.to_lexical(&mut buffer));
        assert_eq!(b"1", 1i64.to_lexical(&mut buffer));
        assert_eq!(b"5", 5i64.to_lexical(&mut buffer));
        assert_eq!(b"9223372036854775807", 9223372036854775807i64.to_lexical(&mut buffer));
        assert_eq!(b"-9223372036854775808", (9223372036854775808u64 as i64).to_lexical(&mut buffer));
        assert_eq!(b"-1", (18446744073709551615u64 as i64).to_lexical(&mut buffer));
        assert_eq!(b"-1", (-1i64).to_lexical(&mut buffer));
    }

    #[test]
    fn u128_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0u128.to_lexical(&mut buffer));
        assert_eq!(b"1", 1u128.to_lexical(&mut buffer));
        assert_eq!(b"5", 5u128.to_lexical(&mut buffer));
        assert_eq!(&b"170141183460469231731687303715884105727"[..], 170141183460469231731687303715884105727u128.to_lexical(&mut buffer));
        assert_eq!(&b"170141183460469231731687303715884105728"[..], 170141183460469231731687303715884105728u128.to_lexical(&mut buffer));
        assert_eq!(&b"340282366920938463463374607431768211455"[..], 340282366920938463463374607431768211455u128.to_lexical(&mut buffer));
        assert_eq!(&b"340282366920938463463374607431768211455"[..], (-1i128 as u128).to_lexical(&mut buffer));
    }

    #[test]
    fn i128_test() {
        let mut buffer = new_buffer();
        assert_eq!(b"0", 0i128.to_lexical(&mut buffer));
        assert_eq!(b"1", 1i128.to_lexical(&mut buffer));
        assert_eq!(b"5", 5i128.to_lexical(&mut buffer));
        assert_eq!(&b"170141183460469231731687303715884105727"[..], 170141183460469231731687303715884105727i128.to_lexical(&mut buffer));
        assert_eq!(&b"-170141183460469231731687303715884105728"[..], (170141183460469231731687303715884105728u128 as i128).to_lexical(&mut buffer));
        assert_eq!(b"-1", (340282366920938463463374607431768211455u128 as i128).to_lexical(&mut buffer));
        assert_eq!(b"-1", (-1i128).to_lexical(&mut buffer));
    }

    #[cfg(feature = "radix")]
    #[test]
    fn radix_test() {
        let data = [
            (2, "100101"),
            (3, "1101"),
            (4, "211"),
            (5, "122"),
            (6, "101"),
            (7, "52"),
            (8, "45"),
            (9, "41"),
            (10, "37"),
            (11, "34"),
            (12, "31"),
            (13, "2B"),
            (14, "29"),
            (15, "27"),
            (16, "25"),
            (17, "23"),
            (18, "21"),
            (19, "1I"),
            (20, "1H"),
            (21, "1G"),
            (22, "1F"),
            (23, "1E"),
            (24, "1D"),
            (25, "1C"),
            (26, "1B"),
            (27, "1A"),
            (28, "19"),
            (29, "18"),
            (30, "17"),
            (31, "16"),
            (32, "15"),
            (33, "14"),
            (34, "13"),
            (35, "12"),
            (36, "11"),
        ];

        let mut buffer = new_buffer();
        for (base, expected) in data.iter() {
            assert_eq!(expected.as_bytes(), 37.to_lexical_radix(*base, &mut buffer));
        }
    }

    // Extensive tests

    #[test]
    fn u8_pow2_test() {
        let mut buffer = new_buffer();
        let values: &[u8] = &[0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255];
        for &i in values.iter() {
            assert_eq!(i, u8::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u8_pow10_test() {
        let mut buffer = new_buffer();
        let values: &[u8] = &[0, 1, 5, 9, 10, 11, 15, 99, 100, 101, 105];
        for &i in values.iter() {
            assert_eq!(i, u8::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u16_pow2_test() {
        let mut buffer = new_buffer();
        let values: &[u16] = &[0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096, 4097, 8191, 8192, 8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535];
        for &i in values.iter() {
            assert_eq!(i, u16::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u16_pow10_test() {
        let mut buffer = new_buffer();
        let values: &[u16] = &[0, 1, 5, 9, 10, 11, 15, 99, 100, 101, 105, 999, 1000, 1001, 1005, 9999, 10000, 10001, 10005];
        for &i in values.iter() {
            assert_eq!(i, u16::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u32_pow2_test() {
        let mut buffer = new_buffer();
        let values: &[u32] = &[0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096, 4097, 8191, 8192, 8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535, 65536, 65537, 131071, 131072, 131073, 262143, 262144, 262145, 524287, 524288, 524289, 1048575, 1048576, 1048577, 2097151, 2097152, 2097153, 4194303, 4194304, 4194305, 8388607, 8388608, 8388609, 16777215, 16777216, 16777217, 33554431, 33554432, 33554433, 67108863, 67108864, 67108865, 134217727, 134217728, 134217729, 268435455, 268435456, 268435457, 536870911, 536870912, 536870913, 1073741823, 1073741824, 1073741825, 2147483647, 2147483648, 2147483649, 4294967295];
        for &i in values.iter() {
            assert_eq!(i, u32::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u32_pow10_test() {
        let mut buffer = new_buffer();
        let values: &[u32] = &[0, 1, 5, 9, 10, 11, 15, 99, 100, 101, 105, 999, 1000, 1001, 1005, 9999, 10000, 10001, 10005, 99999, 100000, 100001, 100005, 999999, 1000000, 1000001, 1000005, 9999999, 10000000, 10000001, 10000005, 99999999, 100000000, 100000001, 100000005, 999999999, 1000000000, 1000000001, 1000000005];
        for &i in values.iter() {
            assert_eq!(i, u32::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u64_pow2_test() {
        let mut buffer = new_buffer();
        let values: &[u64] = &[0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096, 4097, 8191, 8192, 8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535, 65536, 65537, 131071, 131072, 131073, 262143, 262144, 262145, 524287, 524288, 524289, 1048575, 1048576, 1048577, 2097151, 2097152, 2097153, 4194303, 4194304, 4194305, 8388607, 8388608, 8388609, 16777215, 16777216, 16777217, 33554431, 33554432, 33554433, 67108863, 67108864, 67108865, 134217727, 134217728, 134217729, 268435455, 268435456, 268435457, 536870911, 536870912, 536870913, 1073741823, 1073741824, 1073741825, 2147483647, 2147483648, 2147483649, 4294967295, 4294967296, 4294967297, 8589934591, 8589934592, 8589934593, 17179869183, 17179869184, 17179869185, 34359738367, 34359738368, 34359738369, 68719476735, 68719476736, 68719476737, 137438953471, 137438953472, 137438953473, 274877906943, 274877906944, 274877906945, 549755813887, 549755813888, 549755813889, 1099511627775, 1099511627776, 1099511627777, 2199023255551, 2199023255552, 2199023255553, 4398046511103, 4398046511104, 4398046511105, 8796093022207, 8796093022208, 8796093022209, 17592186044415, 17592186044416, 17592186044417, 35184372088831, 35184372088832, 35184372088833, 70368744177663, 70368744177664, 70368744177665, 140737488355327, 140737488355328, 140737488355329, 281474976710655, 281474976710656, 281474976710657, 562949953421311, 562949953421312, 562949953421313, 1125899906842623, 1125899906842624, 1125899906842625, 2251799813685247, 2251799813685248, 2251799813685249, 4503599627370495, 4503599627370496, 4503599627370497, 9007199254740991, 9007199254740992, 9007199254740993, 18014398509481983, 18014398509481984, 18014398509481985, 36028797018963967, 36028797018963968, 36028797018963969, 72057594037927935, 72057594037927936, 72057594037927937, 144115188075855871, 144115188075855872, 144115188075855873, 288230376151711743, 288230376151711744, 288230376151711745, 576460752303423487, 576460752303423488, 576460752303423489, 1152921504606846975, 1152921504606846976, 1152921504606846977, 2305843009213693951, 2305843009213693952, 2305843009213693953, 4611686018427387903, 4611686018427387904, 4611686018427387905, 9223372036854775807, 9223372036854775808, 9223372036854775809, 18446744073709551615];
        for &i in values.iter() {
            assert_eq!(i, u64::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u64_pow10_test() {
        let mut buffer = new_buffer();
        let values: &[u64] = &[0, 1, 5, 9, 10, 11, 15, 99, 100, 101, 105, 999, 1000, 1001, 1005, 9999, 10000, 10001, 10005, 99999, 100000, 100001, 100005, 999999, 1000000, 1000001, 1000005, 9999999, 10000000, 10000001, 10000005, 99999999, 100000000, 100000001, 100000005, 999999999, 1000000000, 1000000001, 1000000005, 9999999999, 10000000000, 10000000001, 10000000005, 99999999999, 100000000000, 100000000001, 100000000005, 999999999999, 1000000000000, 1000000000001, 1000000000005, 9999999999999, 10000000000000, 10000000000001, 10000000000005, 99999999999999, 100000000000000, 100000000000001, 100000000000005, 999999999999999, 1000000000000000, 1000000000000001, 1000000000000005, 9999999999999999, 10000000000000000, 10000000000000001, 10000000000000005, 99999999999999999, 100000000000000000, 100000000000000001, 100000000000000005, 999999999999999999, 1000000000000000000, 1000000000000000001, 1000000000000000005];
        for &i in values.iter() {
            assert_eq!(i, u64::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u128_pow2_test() {
        let mut buffer = new_buffer();
        let values: &[u128] = &[0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096, 4097, 8191, 8192, 8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535, 65536, 65537, 131071, 131072, 131073, 262143, 262144, 262145, 524287, 524288, 524289, 1048575, 1048576, 1048577, 2097151, 2097152, 2097153, 4194303, 4194304, 4194305, 8388607, 8388608, 8388609, 16777215, 16777216, 16777217, 33554431, 33554432, 33554433, 67108863, 67108864, 67108865, 134217727, 134217728, 134217729, 268435455, 268435456, 268435457, 536870911, 536870912, 536870913, 1073741823, 1073741824, 1073741825, 2147483647, 2147483648, 2147483649, 4294967295, 4294967296, 4294967297, 8589934591, 8589934592, 8589934593, 17179869183, 17179869184, 17179869185, 34359738367, 34359738368, 34359738369, 68719476735, 68719476736, 68719476737, 137438953471, 137438953472, 137438953473, 274877906943, 274877906944, 274877906945, 549755813887, 549755813888, 549755813889, 1099511627775, 1099511627776, 1099511627777, 2199023255551, 2199023255552, 2199023255553, 4398046511103, 4398046511104, 4398046511105, 8796093022207, 8796093022208, 8796093022209, 17592186044415, 17592186044416, 17592186044417, 35184372088831, 35184372088832, 35184372088833, 70368744177663, 70368744177664, 70368744177665, 140737488355327, 140737488355328, 140737488355329, 281474976710655, 281474976710656, 281474976710657, 562949953421311, 562949953421312, 562949953421313, 1125899906842623, 1125899906842624, 1125899906842625, 2251799813685247, 2251799813685248, 2251799813685249, 4503599627370495, 4503599627370496, 4503599627370497, 9007199254740991, 9007199254740992, 9007199254740993, 18014398509481983, 18014398509481984, 18014398509481985, 36028797018963967, 36028797018963968, 36028797018963969, 72057594037927935, 72057594037927936, 72057594037927937, 144115188075855871, 144115188075855872, 144115188075855873, 288230376151711743, 288230376151711744, 288230376151711745, 576460752303423487, 576460752303423488, 576460752303423489, 1152921504606846975, 1152921504606846976, 1152921504606846977, 2305843009213693951, 2305843009213693952, 2305843009213693953, 4611686018427387903, 4611686018427387904, 4611686018427387905, 9223372036854775807, 9223372036854775808, 9223372036854775809, 18446744073709551615, 18446744073709551616, 18446744073709551617, 36893488147419103231, 36893488147419103232, 36893488147419103233, 73786976294838206463, 73786976294838206464, 73786976294838206465, 147573952589676412927, 147573952589676412928, 147573952589676412929, 295147905179352825855, 295147905179352825856, 295147905179352825857, 590295810358705651711, 590295810358705651712, 590295810358705651713, 1180591620717411303423, 1180591620717411303424, 1180591620717411303425, 2361183241434822606847, 2361183241434822606848, 2361183241434822606849, 4722366482869645213695, 4722366482869645213696, 4722366482869645213697, 9444732965739290427391, 9444732965739290427392, 9444732965739290427393, 18889465931478580854783, 18889465931478580854784, 18889465931478580854785, 37778931862957161709567, 37778931862957161709568, 37778931862957161709569, 75557863725914323419135, 75557863725914323419136, 75557863725914323419137, 151115727451828646838271, 151115727451828646838272, 151115727451828646838273, 302231454903657293676543, 302231454903657293676544, 302231454903657293676545, 604462909807314587353087, 604462909807314587353088, 604462909807314587353089, 1208925819614629174706175, 1208925819614629174706176, 1208925819614629174706177, 2417851639229258349412351, 2417851639229258349412352, 2417851639229258349412353, 4835703278458516698824703, 4835703278458516698824704, 4835703278458516698824705, 9671406556917033397649407, 9671406556917033397649408, 9671406556917033397649409, 19342813113834066795298815, 19342813113834066795298816, 19342813113834066795298817, 38685626227668133590597631, 38685626227668133590597632, 38685626227668133590597633, 77371252455336267181195263, 77371252455336267181195264, 77371252455336267181195265, 154742504910672534362390527, 154742504910672534362390528, 154742504910672534362390529, 309485009821345068724781055, 309485009821345068724781056, 309485009821345068724781057, 618970019642690137449562111, 618970019642690137449562112, 618970019642690137449562113, 1237940039285380274899124223, 1237940039285380274899124224, 1237940039285380274899124225, 2475880078570760549798248447, 2475880078570760549798248448, 2475880078570760549798248449, 4951760157141521099596496895, 4951760157141521099596496896, 4951760157141521099596496897, 9903520314283042199192993791, 9903520314283042199192993792, 9903520314283042199192993793, 19807040628566084398385987583, 19807040628566084398385987584, 19807040628566084398385987585, 39614081257132168796771975167, 39614081257132168796771975168, 39614081257132168796771975169, 79228162514264337593543950335, 79228162514264337593543950336, 79228162514264337593543950337, 158456325028528675187087900671, 158456325028528675187087900672, 158456325028528675187087900673, 316912650057057350374175801343, 316912650057057350374175801344, 316912650057057350374175801345, 633825300114114700748351602687, 633825300114114700748351602688, 633825300114114700748351602689, 1267650600228229401496703205375, 1267650600228229401496703205376, 1267650600228229401496703205377, 2535301200456458802993406410751, 2535301200456458802993406410752, 2535301200456458802993406410753, 5070602400912917605986812821503, 5070602400912917605986812821504, 5070602400912917605986812821505, 10141204801825835211973625643007, 10141204801825835211973625643008, 10141204801825835211973625643009, 20282409603651670423947251286015, 20282409603651670423947251286016, 20282409603651670423947251286017, 40564819207303340847894502572031, 40564819207303340847894502572032, 40564819207303340847894502572033, 81129638414606681695789005144063, 81129638414606681695789005144064, 81129638414606681695789005144065, 162259276829213363391578010288127, 162259276829213363391578010288128, 162259276829213363391578010288129, 324518553658426726783156020576255, 324518553658426726783156020576256, 324518553658426726783156020576257, 649037107316853453566312041152511, 649037107316853453566312041152512, 649037107316853453566312041152513, 1298074214633706907132624082305023, 1298074214633706907132624082305024, 1298074214633706907132624082305025, 2596148429267413814265248164610047, 2596148429267413814265248164610048, 2596148429267413814265248164610049, 5192296858534827628530496329220095, 5192296858534827628530496329220096, 5192296858534827628530496329220097, 10384593717069655257060992658440191, 10384593717069655257060992658440192, 10384593717069655257060992658440193, 20769187434139310514121985316880383, 20769187434139310514121985316880384, 20769187434139310514121985316880385, 41538374868278621028243970633760767, 41538374868278621028243970633760768, 41538374868278621028243970633760769, 83076749736557242056487941267521535, 83076749736557242056487941267521536, 83076749736557242056487941267521537, 166153499473114484112975882535043071, 166153499473114484112975882535043072, 166153499473114484112975882535043073, 332306998946228968225951765070086143, 332306998946228968225951765070086144, 332306998946228968225951765070086145, 664613997892457936451903530140172287, 664613997892457936451903530140172288, 664613997892457936451903530140172289, 1329227995784915872903807060280344575, 1329227995784915872903807060280344576, 1329227995784915872903807060280344577, 2658455991569831745807614120560689151, 2658455991569831745807614120560689152, 2658455991569831745807614120560689153, 5316911983139663491615228241121378303, 5316911983139663491615228241121378304, 5316911983139663491615228241121378305, 10633823966279326983230456482242756607, 10633823966279326983230456482242756608, 10633823966279326983230456482242756609, 21267647932558653966460912964485513215, 21267647932558653966460912964485513216, 21267647932558653966460912964485513217, 42535295865117307932921825928971026431, 42535295865117307932921825928971026432, 42535295865117307932921825928971026433, 85070591730234615865843651857942052863, 85070591730234615865843651857942052864, 85070591730234615865843651857942052865, 170141183460469231731687303715884105727, 170141183460469231731687303715884105728, 170141183460469231731687303715884105729, 340282366920938463463374607431768211455];
        for &i in values.iter() {
            assert_eq!(i, u128::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    fn u128_pow10_test() {
        let mut buffer = new_buffer();
        let values: &[u128] = &[0, 1, 5, 9, 10, 11, 15, 99, 100, 101, 105, 999, 1000, 1001, 1005, 9999, 10000, 10001, 10005, 99999, 100000, 100001, 100005, 999999, 1000000, 1000001, 1000005, 9999999, 10000000, 10000001, 10000005, 99999999, 100000000, 100000001, 100000005, 999999999, 1000000000, 1000000001, 1000000005, 9999999999, 10000000000, 10000000001, 10000000005, 99999999999, 100000000000, 100000000001, 100000000005, 999999999999, 1000000000000, 1000000000001, 1000000000005, 9999999999999, 10000000000000, 10000000000001, 10000000000005, 99999999999999, 100000000000000, 100000000000001, 100000000000005, 999999999999999, 1000000000000000, 1000000000000001, 1000000000000005, 9999999999999999, 10000000000000000, 10000000000000001, 10000000000000005, 99999999999999999, 100000000000000000, 100000000000000001, 100000000000000005, 999999999999999999, 1000000000000000000, 1000000000000000001, 1000000000000000005, 9999999999999999999, 10000000000000000000, 10000000000000000001, 10000000000000000005, 99999999999999999999, 100000000000000000000, 100000000000000000001, 100000000000000000005, 999999999999999999999, 1000000000000000000000, 1000000000000000000001, 1000000000000000000005, 9999999999999999999999, 10000000000000000000000, 10000000000000000000001, 10000000000000000000005, 99999999999999999999999, 100000000000000000000000, 100000000000000000000001, 100000000000000000000005, 999999999999999999999999, 1000000000000000000000000, 1000000000000000000000001, 1000000000000000000000005, 9999999999999999999999999, 10000000000000000000000000, 10000000000000000000000001, 10000000000000000000000005, 99999999999999999999999999, 100000000000000000000000000, 100000000000000000000000001, 100000000000000000000000005, 999999999999999999999999999, 1000000000000000000000000000, 1000000000000000000000000001, 1000000000000000000000000005, 9999999999999999999999999999, 10000000000000000000000000000, 10000000000000000000000000001, 10000000000000000000000000005, 99999999999999999999999999999, 100000000000000000000000000000, 100000000000000000000000000001, 100000000000000000000000000005, 999999999999999999999999999999, 1000000000000000000000000000000, 1000000000000000000000000000001, 1000000000000000000000000000005, 9999999999999999999999999999999, 10000000000000000000000000000000, 10000000000000000000000000000001, 10000000000000000000000000000005, 99999999999999999999999999999999, 100000000000000000000000000000000, 100000000000000000000000000000001, 100000000000000000000000000000005, 999999999999999999999999999999999, 1000000000000000000000000000000000, 1000000000000000000000000000000001, 1000000000000000000000000000000005, 9999999999999999999999999999999999, 10000000000000000000000000000000000, 10000000000000000000000000000000001, 10000000000000000000000000000000005, 99999999999999999999999999999999999, 100000000000000000000000000000000000, 100000000000000000000000000000000001, 100000000000000000000000000000000005, 999999999999999999999999999999999999, 1000000000000000000000000000000000000, 1000000000000000000000000000000000001, 1000000000000000000000000000000000005, 9999999999999999999999999999999999999, 10000000000000000000000000000000000000, 10000000000000000000000000000000000001, 10000000000000000000000000000000000005, 99999999999999999999999999999999999999, 100000000000000000000000000000000000000, 100000000000000000000000000000000000001, 100000000000000000000000000000000000005];
        for &i in values.iter() {
            assert_eq!(i, u128::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    // Quickcheck

    quickcheck! {
        fn u8_quickcheck(i: u8) -> bool {
            let mut buffer = new_buffer();
            i == u8::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn u16_quickcheck(i: u16) -> bool {
            let mut buffer = new_buffer();
            i == u16::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn u32_quickcheck(i: u32) -> bool {
            let mut buffer = new_buffer();
            i == u32::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn u64_quickcheck(i: u64) -> bool {
            let mut buffer = new_buffer();
            i == u64::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn u128_quickcheck(i: u128) -> bool {
            let mut buffer = new_buffer();
            i == u128::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn usize_quickcheck(i: usize) -> bool {
            let mut buffer = new_buffer();
            i == usize::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn i8_quickcheck(i: i8) -> bool {
            let mut buffer = new_buffer();
            i == i8::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn i16_quickcheck(i: i16) -> bool {
            let mut buffer = new_buffer();
            i == i16::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn i32_quickcheck(i: i32) -> bool {
            let mut buffer = new_buffer();
            i == i32::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn i64_quickcheck(i: i64) -> bool {
            let mut buffer = new_buffer();
            i == i64::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn i128_quickcheck(i: i128) -> bool {
            let mut buffer = new_buffer();
            i == i128::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }

        fn isize_quickcheck(i: isize) -> bool {
            let mut buffer = new_buffer();
            i == isize::from_lexical(i.to_lexical(&mut buffer)).unwrap()
        }
    }

    // Proptest

    #[cfg(feature = "std")]
    proptest! {
        #[test]
        fn u8_proptest(i in u8::min_value()..u8::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, u8::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn i8_proptest(i in i8::min_value()..i8::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, i8::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn u16_proptest(i in u16::min_value()..u16::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, u16::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn i16_proptest(i in i16::min_value()..i16::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, i16::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn u32_proptest(i in u32::min_value()..u32::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, u32::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn i32_proptest(i in i32::min_value()..i32::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, i32::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn u64_proptest(i in u64::min_value()..u64::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, u64::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn i64_proptest(i in i64::min_value()..i64::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, i64::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn u128_proptest(i in u128::min_value()..u128::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, u128::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn i128_proptest(i in i128::min_value()..i128::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, i128::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn usize_proptest(i in usize::min_value()..usize::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, usize::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }

        #[test]
        fn isize_proptest(i in isize::min_value()..isize::max_value()) {
            let mut buffer = new_buffer();
            assert_eq!(i, isize::from_lexical(i.to_lexical(&mut buffer)).unwrap())
        }
    }

    // Panic tests

    #[test]
    #[should_panic]
    fn i8_buffer_test() {
        let mut buffer = [b'0'; i8::FORMATTED_SIZE_DECIMAL-1];
        12i8.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn i16_buffer_test() {
        let mut buffer = [b'0'; i16::FORMATTED_SIZE_DECIMAL-1];
        12i16.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn i32_buffer_test() {
        let mut buffer = [b'0'; i32::FORMATTED_SIZE_DECIMAL-1];
        12i32.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn i64_buffer_test() {
        let mut buffer = [b'0'; i64::FORMATTED_SIZE_DECIMAL-1];
        12i64.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn i128_buffer_test() {
        let mut buffer = [b'0'; i128::FORMATTED_SIZE_DECIMAL-1];
        12i128.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn isize_buffer_test() {
        let mut buffer = [b'0'; isize::FORMATTED_SIZE_DECIMAL-1];
        12isize.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn u8_buffer_test() {
        let mut buffer = [b'0'; u8::FORMATTED_SIZE_DECIMAL-1];
        12i8.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn u16_buffer_test() {
        let mut buffer = [b'0'; u16::FORMATTED_SIZE_DECIMAL-1];
        12i16.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn u32_buffer_test() {
        let mut buffer = [b'0'; u32::FORMATTED_SIZE_DECIMAL-1];
        12i32.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn u64_buffer_test() {
        let mut buffer = [b'0'; u64::FORMATTED_SIZE_DECIMAL-1];
        12i64.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn u128_buffer_test() {
        let mut buffer = [b'0'; u128::FORMATTED_SIZE_DECIMAL-1];
        12i128.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn usize_buffer_test() {
        let mut buffer = [b'0'; usize::FORMATTED_SIZE_DECIMAL-1];
        12usize.to_lexical(&mut buffer);
    }
}
