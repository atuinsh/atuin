//! Slow, simple lexical integer-to-string conversion routine.

use crate::util::*;

// Naive itoa algorithm.
macro_rules! naive_algorithm {
    ($value:ident, $radix:ident, $buffer:ident, $index:ident) => ({
        while $value >= $radix {
            let r = ($value % $radix).as_usize();
            $value /= $radix;

            // This is always safe, since r must be [0, radix).
            $index -= 1;
            unchecked_index_mut!($buffer[$index] = digit_to_char(r));
        }

        // Decode last digit.
        let r = ($value % $radix).as_usize();
        // This is always safe, since r must be [0, radix).
        $index -= 1;
        unchecked_index_mut!($buffer[$index] = digit_to_char(r));
    });
}

// Naive implementation for radix-N numbers.
// Precondition: `value` must be non-negative and mutable.
perftools_inline!{
fn naive<T>(mut value: T, radix: u32, buffer: &mut [u8])
    -> usize
    where T: UnsignedInteger
{
    // Decode all but last digit, 1 at a time.
    let mut index = buffer.len();
    let radix: T = as_cast(radix);
    naive_algorithm!(value, radix, buffer, index);
    index
}}

pub(crate) trait Naive {
    // Export integer to string.
    fn naive(self, radix: u32, buffer: &mut [u8]) -> usize;
}

// Implement naive for type.
macro_rules! naive_impl {
    ($($t:ty)*) => ($(
        impl Naive for $t {
            perftools_inline_always!{
            fn naive(self, radix: u32, buffer: &mut [u8]) -> usize {
                naive(self, radix, buffer)
            }}
        }
    )*);
}

naive_impl! { u8 u16 u32 u64 usize }

// Naive implementation for 128-bit radix-N numbers.
// Precondition: `value` must be non-negative and mutable.
perftools_inline!{
fn naive_u128(value: u128, radix: u32, buffer: &mut [u8])
    -> usize
{
    // Decode all but last digit, 1 at a time.
    let (divisor, digits_per_iter, d_cltz) = u128_divisor(radix);
    let radix: u64 = as_cast(radix);

    // To deal with internal 0 values or values with internal 0 digits set,
    // we store the starting index, and if not all digits are written,
    // we just skip down `digits` digits for the next value.
    let mut index = buffer.len();
    let mut start_index = index;
    let (value, mut low) = u128_divrem(value, divisor, d_cltz);
    naive_algorithm!(low, radix, buffer, index);
    if value != 0 {
        start_index -= digits_per_iter;
        index = index.min(start_index);
        let (value, mut mid) = u128_divrem(value, divisor, d_cltz);
        naive_algorithm!(mid, radix, buffer, index);

        if value != 0 {
            start_index -= digits_per_iter;
            index = index.min(start_index);
            let mut high = value as u64;
            naive_algorithm!(high, radix, buffer, index);
        }
    }
    index
}}

impl Naive for u128 {
    perftools_inline_always!{
    fn naive(self, radix: u32, buffer: &mut [u8]) -> usize {
        naive_u128(self, radix, buffer)
    }}
}
