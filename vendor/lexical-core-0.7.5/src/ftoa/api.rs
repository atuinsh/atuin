//! Low-level API generator.
//!
//! Uses either the internal "Grisu2", or the external "Grisu3" or "Ryu"
//! algorithms provided by `https://github.com/dtolnay`.

//  The following benchmarks were run on an "Intel(R) Core(TM) i7-6560U
//  CPU @ 2.20GHz" CPU, on Fedora 28, Linux kernel version 4.18.16-200
//  (x86-64), using the lexical formatter or `x.parse()`,
//  avoiding any inefficiencies in Rust string parsing. The code was
//  compiled with LTO and at an optimization level of 3.
//
//  The benchmarks with `std` were compiled using "rustc 1.29.2 (17a9dc751
//  2018-10-05", and the `no_std` benchmarks were compiled using "rustc
//  1.31.0-nightly (46880f41b 2018-10-15)".
//
//  The benchmark code may be found `benches/atof.rs`.
//
//  # Benchmarks
//
//  | Type  |  lexical (ns/iter) | libcore (ns/iter)     | Relative Increase |
//  |:-----:|:------------------:|:---------------------:|:-----------------:|
//  | f32   | 465,584            | 1,884,646             | 4.04x             |
//  | f64   | 539,904            | 2,276,839             | 4.22x             |
//
//  # Raw Benchmarks
//
//  ```text
//  test ftoa_f32_dtoa    ... bench:     917,561 ns/iter (+/- 45,458)
//  test ftoa_f32_lexical ... bench:     465,584 ns/iter (+/- 76,158)
//  test ftoa_f32_std     ... bench:   1,884,646 ns/iter (+/- 130,721)
//  test ftoa_f64_dtoa    ... bench:   1,092,687 ns/iter (+/- 125,136)
//  test ftoa_f64_lexical ... bench:     539,904 ns/iter (+/- 29,626)
//  test ftoa_f64_std     ... bench:   2,276,839 ns/iter (+/- 64,515)
//  ```

// Code the generate the benchmark plot:
//  import numpy as np
//  import pandas as pd
//  import matplotlib.pyplot as plt
//  plt.style.use('ggplot')
//  lexical = np.array([465584, 539904]) / 1e6
//  rustcore = np.array([1884646, 2276839]) / 1e6
//  dtoa = np.array([917561, 1092687]) / 1e6
//  ryu = np.array([432878, 522515]) / 1e6
//  index = ["f32", "f64"]
//  df = pd.DataFrame({'lexical': lexical, 'rustcore': rustcore, 'dtoa': dtoa, 'ryu': ryu}, index = index, columns=['lexical', 'dtoa', 'ryu', 'rustcore'])
//  ax = df.plot.bar(rot=0, figsize=(16, 8), fontsize=14)
//  ax.set_ylabel("ms/iter")
//  ax.figure.tight_layout()
//  ax.legend(loc=2, prop={'size': 14})
//  plt.show()

use crate::util::*;

#[cfg(feature = "radix")]
use super::radix::{double_radix, float_radix};

// Select the back-end
cfg_if! {
if #[cfg(feature = "grisu3")] {
    use super::grisu3::{double_decimal, float_decimal};
} else if #[cfg(feature = "ryu")] {
    use super::ryu::{double_decimal, float_decimal};
} else {
    use super::grisu2::{double_decimal, float_decimal};
}}  //cfg_if

// TRAITS

/// Trait to define serialization of a float to string.
pub(crate) trait FloatToString: Float {
    /// Export float to decimal string with optimized algorithm.
    fn decimal<'a>(self, bytes: &'a mut [u8]) -> usize;

    /// Export float to radix string with slow algorithm.
    #[cfg(feature = "radix")]
    fn radix<'a>(self, radix: u32, bytes: &'a mut [u8]) -> usize;
}

impl FloatToString for f32 {
    perftools_inline!{
    fn decimal<'a>(self, bytes: &'a mut [u8]) -> usize {
        float_decimal(self, bytes)
    }}

    perftools_inline!{
    #[cfg(feature = "radix")]
    fn radix<'a>(self, radix: u32, bytes: &'a mut [u8]) -> usize {
        float_radix(self, radix, bytes)
    }}
}

impl FloatToString for f64 {
    perftools_inline!{
    fn decimal<'a>(self, bytes: &'a mut [u8]) -> usize {
        double_decimal(self, bytes)
    }}

    perftools_inline!{
    #[cfg(feature = "radix")]
    fn radix<'a>(self, radix: u32, bytes: &'a mut [u8]) -> usize {
        double_radix(self, radix, bytes)
    }}
}

// FTOA

// Forward the correct arguments the ideal encoder.
perftools_inline!{
fn forward<'a, F: FloatToString>(value: F, radix: u32, bytes: &'a mut [u8])
    -> usize
{
    debug_assert_radix!(radix);

    #[cfg(not(feature = "radix"))] {
        value.decimal(bytes)
    }

    #[cfg(feature = "radix")] {
        match radix {
            10 => value.decimal(bytes),
            _  => value.radix(radix, bytes),
        }
    }
}}

// Convert float-to-string and handle special (positive) floats.
perftools_inline!{
fn filter_special<'a, F: FloatToString>(value: F, radix: u32, bytes: &'a mut [u8])
    -> usize
{
    // Logic errors, disable in release builds.
    debug_assert!(value.is_sign_positive(), "Value cannot be negative.");
    debug_assert_radix!(radix);

    // We already check for 0 in `filter_sign` if value.is_zero().
    #[cfg(not(feature = "trim_floats"))] {
        if value.is_zero() {
            // This is safe, because we confirmed the buffer is >= 4
            // in total (since we also handled the sign by here).
            return copy_to_dst(bytes, b"0.0");
        }
    }

    if value.is_nan() {
        // This is safe, because we confirmed the buffer is >= F::FORMATTED_SIZE.
        // We have up to `F::FORMATTED_SIZE - 1` bytes from `get_nan_string()`,
        // and up to 1 byte from the sign.
        copy_to_dst(bytes, get_nan_string())
    } else if value.is_special() {
        // This is safe, because we confirmed the buffer is >= F::FORMATTED_SIZE.
        // We have up to `F::FORMATTED_SIZE - 1` bytes from `get_inf_string()`,
        // and up to 1 byte from the sign.
        copy_to_dst(bytes, get_inf_string())
    } else {
        forward(value, radix, bytes)
    }
}}

// Handle +/- values.
perftools_inline!{
fn filter_sign<'a, F: FloatToString>(value: F, radix: u32, bytes: &'a mut [u8])
    -> usize
{
    debug_assert_radix!(radix);

    // Export "-0.0" and "0.0" as "0" with trimmed floats.
    #[cfg(feature = "trim_floats")] {
        if value.is_zero() {
            // We know this is safe, because we confirmed the buffer is >= 1.
            index_mut!(bytes[0] = b'0');
            return 1;
        }
    }

    // If the sign bit is set, invert it and just set the first
    // value to "-".
    if value.is_sign_negative() {
        let value = -value;
        // We know this is safe, because we confirmed the buffer is >= 1.
        index_mut!(bytes[0] = b'-');
        let bytes = &mut index_mut!(bytes[1..]);
        filter_special(value, radix, bytes) + 1
    } else {
        filter_special(value, radix, bytes)
    }
}}

// Write float to string..
perftools_inline!{
fn ftoa<F: FloatToString>(value: F, radix: u32, bytes: &mut [u8])
    -> usize
{
    let len = filter_sign(value, radix, bytes);
    let bytes = &mut index_mut!(bytes[..len]);
    trim(bytes)
}}

// Trim a trailing ".0" from a float.
perftools_inline!{
fn trim<'a>(bytes: &'a mut [u8])
    -> usize
{
    // Trim a trailing ".0" from a float.
    if cfg!(feature = "trim_floats") && ends_with_slice(bytes, b".0") {
        bytes.len() - 2
    } else {
        bytes.len()
    }
}}

// TO LEXICAL

to_lexical!(ftoa, f32);
to_lexical!(ftoa, f64);

// TESTS
// -----

#[cfg(test)]
mod tests {
    use crate::util::*;
    use crate::util::test::*;

    // Test data for roundtrips.
    const F32_DATA : [f32; 31] = [0., 0.1, 1., 1.1, 12., 12.1, 123., 123.1, 1234., 1234.1, 12345., 12345.1, 123456., 123456.1, 1234567., 1234567.1, 12345678., 12345678.1, 123456789., 123456789.1, 123456789.12, 123456789.123, 123456789.1234, 123456789.12345, 1.2345678912345e8, 1.2345e+8, 1.2345e+11, 1.2345e+38, 1.2345e-8, 1.2345e-11, 1.2345e-38];
    const F64_DATA: [f64; 33] = [0., 0.1, 1., 1.1, 12., 12.1, 123., 123.1, 1234., 1234.1, 12345., 12345.1, 123456., 123456.1, 1234567., 1234567.1, 12345678., 12345678.1, 123456789., 123456789.1, 123456789.12, 123456789.123, 123456789.1234, 123456789.12345, 1.2345678912345e8, 1.2345e+8, 1.2345e+11, 1.2345e+38, 1.2345e+308, 1.2345e-8, 1.2345e-11, 1.2345e-38, 1.2345e-299];

    #[cfg(feature = "radix")]
    #[test]
    fn f32_binary_test() {
        let mut buffer = new_buffer();
        // positive
        #[cfg(feature = "trim_floats")] {
            assert_eq!(as_slice(b"0"), 0.0f32.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"0"), (-0.0f32).to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"1"), 1.0f32.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"10"), 2.0f32.to_lexical_radix(2, &mut buffer));
        }

        #[cfg(not(feature = "trim_floats"))] {
            assert_eq!(as_slice(b"0.0"), 0.0f32.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"-0.0"), (-0.0f32).to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"1.0"), 1.0f32.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"10.0"), 2.0f32.to_lexical_radix(2, &mut buffer));
        }

        assert_eq!(as_slice(b"1.1"), 1.5f32.to_lexical_radix(2, &mut buffer));
        assert_eq!(as_slice(b"1.01"), 1.25f32.to_lexical_radix(2, &mut buffer));
        assert_eq!(b"1.001111000000110010", &1.2345678901234567890e0f32.to_lexical_radix(2, &mut buffer)[..20]);
        assert_eq!(b"1100.010110000111111", &1.2345678901234567890e1f32.to_lexical_radix(2, &mut buffer)[..20]);
        assert_eq!(b"1111011.011101001111", &1.2345678901234567890e2f32.to_lexical_radix(2, &mut buffer)[..20]);
        assert_eq!(b"10011010010.10010001", &1.2345678901234567890e3f32.to_lexical_radix(2, &mut buffer)[..20]);

        // negative
        assert_eq!(b"-1.001111000000110010", &(-1.2345678901234567890e0f32).to_lexical_radix(2, &mut buffer)[..21]);
        assert_eq!(b"-1100.010110000111111", &(-1.2345678901234567890e1f32).to_lexical_radix(2, &mut buffer)[..21]);
        assert_eq!(b"-1111011.011101001111", &(-1.2345678901234567890e2f32).to_lexical_radix(2, &mut buffer)[..21]);
        assert_eq!(b"-10011010010.10010001", &(-1.2345678901234567890e3f32).to_lexical_radix(2, &mut buffer)[..21]);

        // special
        assert_eq!(as_slice(b"NaN"), f32::NAN.to_lexical_radix(2, &mut buffer));
        assert_eq!(as_slice(b"inf"), f32::INFINITY.to_lexical_radix(2, &mut buffer));

        // bugfixes
        assert_eq!(as_slice(b"1.1010100000101011110001e-11011"), 0.000000012345f32.to_lexical_radix(2, &mut buffer));
    }

    #[test]
    fn f32_decimal_test() {
        let mut buffer = new_buffer();
        // positive
        #[cfg(feature = "trim_floats")] {
            assert_eq!(as_slice(b"0"), 0.0f32.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"0"), (-0.0f32).to_lexical(&mut buffer));
            assert_eq!(as_slice(b"1"), 1.0f32.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"10"), 10.0f32.to_lexical(&mut buffer));
        }

        #[cfg(not(feature = "trim_floats"))] {
            assert_eq!(as_slice(b"0.0"), 0.0f32.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"-0.0"), (-0.0f32).to_lexical(&mut buffer));
            assert_eq!(as_slice(b"1.0"), 1.0f32.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"10.0"), 10.0f32.to_lexical(&mut buffer));
        }

        assert_eq!(as_slice(b"1.234567"), &1.2345678901234567890e0f32.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"12.34567"), &1.2345678901234567890e1f32.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"123.4567"), &1.2345678901234567890e2f32.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"1234.567"), &1.2345678901234567890e3f32.to_lexical(&mut buffer)[..8]);

        // negative
        assert_eq!(as_slice(b"-1.234567"), &(-1.2345678901234567890e0f32).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-12.34567"), &(-1.2345678901234567890e1f32).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-123.4567"), &(-1.2345678901234567890e2f32).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-1234.567"), &(-1.2345678901234567890e3f32).to_lexical(&mut buffer)[..9]);

        // special
        assert_eq!(as_slice(b"NaN"), f32::NAN.to_lexical(&mut buffer));
        assert_eq!(as_slice(b"inf"), f32::INFINITY.to_lexical(&mut buffer));
    }

    #[test]
    fn f32_decimal_roundtrip_test() {
        let mut buffer = new_buffer();
        for &f in F32_DATA.iter() {
            let s = f.to_lexical(&mut buffer);
            assert_relative_eq!(f32::from_lexical(s).unwrap(), f, epsilon=1e-6, max_relative=1e-6);
        }
    }

    #[cfg(feature = "radix")]
    #[test]
    fn f32_radix_roundtrip_test() {
        let mut buffer = new_buffer();
        for &f in F32_DATA.iter() {
            for radix in 2..37 {
                // The lower accuracy is due to slight rounding errors of
                // ftoa for the Grisu method with non-10 bases.
                let s = f.to_lexical_radix(radix, &mut buffer);
                assert_relative_eq!(f32::from_lexical_radix(s, radix).unwrap(), f, max_relative=2e-5);
            }
        }
    }

    #[cfg(feature = "radix")]
    #[test]
    fn f64_binary_test() {
        let mut buffer = new_buffer();
        // positive
        #[cfg(feature = "trim_floats")] {
            assert_eq!(as_slice(b"0"), 0.0f64.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"0"), (-0.0f64).to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"1"), 1.0f64.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"10"), 2.0f64.to_lexical_radix(2, &mut buffer));
        }

        #[cfg(not(feature = "trim_floats"))] {
            assert_eq!(as_slice(b"0.0"), 0.0f64.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"-0.0"), (-0.0f64).to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"1.0"), 1.0f64.to_lexical_radix(2, &mut buffer));
            assert_eq!(as_slice(b"10.0"), 2.0f64.to_lexical_radix(2, &mut buffer));
        }

        assert_eq!(as_slice(b"1.00111100000011001010010000101000110001"), &1.2345678901234567890e0f64.to_lexical_radix(2, &mut buffer)[..40]);
        assert_eq!(as_slice(b"1100.01011000011111100110100110010111101"), &1.2345678901234567890e1f64.to_lexical_radix(2, &mut buffer)[..40]);
        assert_eq!(as_slice(b"1111011.01110100111100000001111111101101"), &1.2345678901234567890e2f64.to_lexical_radix(2, &mut buffer)[..40]);
        assert_eq!(as_slice(b"10011010010.1001000101100001001111110100"), &1.2345678901234567890e3f64.to_lexical_radix(2, &mut buffer)[..40]);

        // negative
        assert_eq!(as_slice(b"-1.00111100000011001010010000101000110001"), &(-1.2345678901234567890e0f64).to_lexical_radix(2, &mut buffer)[..41]);
        assert_eq!(as_slice(b"-1100.01011000011111100110100110010111101"), &(-1.2345678901234567890e1f64).to_lexical_radix(2, &mut buffer)[..41]);
        assert_eq!(as_slice(b"-1111011.01110100111100000001111111101101"), &(-1.2345678901234567890e2f64).to_lexical_radix(2, &mut buffer)[..41]);
        assert_eq!(as_slice(b"-10011010010.1001000101100001001111110100"), &(-1.2345678901234567890e3f64).to_lexical_radix(2, &mut buffer)[..41]);

        // special
        assert_eq!(as_slice(b"NaN"), f64::NAN.to_lexical_radix(2, &mut buffer));
        assert_eq!(as_slice(b"inf"), f64::INFINITY.to_lexical_radix(2, &mut buffer));
    }

    #[test]
    fn f64_decimal_test() {
        let mut buffer = new_buffer();
        // positive
        #[cfg(feature = "trim_floats")] {
            assert_eq!(as_slice(b"0"), 0.0.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"0"), (-0.0).to_lexical(&mut buffer));
            assert_eq!(as_slice(b"1"), 1.0.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"10"), 10.0.to_lexical(&mut buffer));
        }

        #[cfg(not(feature = "trim_floats"))] {
            assert_eq!(as_slice(b"0.0"), 0.0.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"-0.0"), (-0.0).to_lexical(&mut buffer));
            assert_eq!(as_slice(b"1.0"), 1.0.to_lexical(&mut buffer));
            assert_eq!(as_slice(b"10.0"), 10.0.to_lexical(&mut buffer));
        }

        assert_eq!(as_slice(b"1.234567"), &1.2345678901234567890e0.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"12.34567"), &1.2345678901234567890e1.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"123.4567"), &1.2345678901234567890e2.to_lexical(&mut buffer)[..8]);
        assert_eq!(as_slice(b"1234.567"), &1.2345678901234567890e3.to_lexical(&mut buffer)[..8]);

        // negative
        assert_eq!(as_slice(b"-1.234567"), &(-1.2345678901234567890e0).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-12.34567"), &(-1.2345678901234567890e1).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-123.4567"), &(-1.2345678901234567890e2).to_lexical(&mut buffer)[..9]);
        assert_eq!(as_slice(b"-1234.567"), &(-1.2345678901234567890e3).to_lexical(&mut buffer)[..9]);

        // special
        assert_eq!(b"NaN".to_vec(), f64::NAN.to_lexical(&mut buffer));
        assert_eq!(b"inf".to_vec(), f64::INFINITY.to_lexical(&mut buffer));
    }

    #[test]
    fn f64_decimal_roundtrip_test() {
        let mut buffer = new_buffer();
        for &f in F64_DATA.iter() {
            let s = f.to_lexical(&mut buffer);
            assert_relative_eq!(f64::from_lexical(s).unwrap(), f, epsilon=1e-12, max_relative=1e-12);
        }
    }

    #[cfg(feature = "radix")]
    #[test]
    fn f64_radix_roundtrip_test() {
        let mut buffer = new_buffer();
        for &f in F64_DATA.iter() {
            for radix in 2..37 {
                // The lower accuracy is due to slight rounding errors of
                // ftoa for the Grisu method with non-10 bases.
                let s = f.to_lexical_radix(radix, &mut buffer);
                assert_relative_eq!(f64::from_lexical_radix(s, radix).unwrap(), f, max_relative=3e-5);
            }
        }
    }

    #[cfg(feature = "correct")]
    quickcheck! {
        fn f32_quickcheck(f: f32) -> bool {
            let mut buffer = new_buffer();
            f == f32::from_lexical(f.to_lexical(&mut buffer)).unwrap()
        }

        fn f64_quickcheck(f: f64) -> bool {
            let mut buffer = new_buffer();
            f == f64::from_lexical(f.to_lexical(&mut buffer)).unwrap()
        }
    }

    #[cfg(all(feature = "correct", feature = "std"))]
    proptest! {
        #[test]
        fn f32_proptest(i in f32::MIN..f32::MAX) {
            let mut buffer = new_buffer();
            prop_assert_eq!(i, f32::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }

        #[test]
        fn f64_proptest(i in f64::MIN..f64::MAX) {
            let mut buffer = new_buffer();
            prop_assert_eq!(i, f64::from_lexical(i.to_lexical(&mut buffer)).unwrap());
        }
    }

    #[test]
    #[should_panic]
    fn f32_buffer_test() {
        let mut buffer = [b'0'; f32::FORMATTED_SIZE_DECIMAL-1];
        1.2345f32.to_lexical(&mut buffer);
    }

    #[test]
    #[should_panic]
    fn f64_buffer_test() {
        let mut buffer = [b'0'; f64::FORMATTED_SIZE_DECIMAL-1];
        1.2345f64.to_lexical(&mut buffer);
    }
}
