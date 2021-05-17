//! Correct algorithms for string-to-float conversions.
//!
//! This implementation is loosely based off the Golang implementation,
//! found here:
//!     https://golang.org/src/strconv/atof.go

use crate::atoi;
use crate::float::*;
use crate::util::*;
use super::alias::*;
use super::bhcomp;
use super::cached::ModeratePathCache;
use super::errors::FloatErrors;
use super::format::*;
use super::small_powers::get_small_powers_64;

// HELPERS
// -------

// Parse the raw float state into a mantissa, calculating the number
// of truncated digits and the offset.
perftools_inline!{
fn process_mantissa<'a, M, Data>(data: &Data, radix: u32)
    -> (M, usize)
    where M: Mantissa,
          Data: FastDataInterface<'a>
{
    atoi::standalone_mantissa(data.integer_iter(), data.fraction_iter(), radix)
}}

// FAST
// ----

// POWN

/// Convert mantissa to exact value for a non-base2 power.
///
/// Returns the resulting float and if the value can be represented exactly.
fn fast_path<F>(mantissa: u64, radix: u32, exponent: i32)
    -> Option<F>
    where F: FloatType
{
    debug_assert_radix!(radix);
    debug_assert!(pow2_exponent(radix) == 0, "Cannot use `fast_path` with a power of 2.");

    // `mantissa >> (F::MANTISSA_SIZE+1) != 0` effectively checks if the
    // value has a no bits above the hidden bit, which is what we want.
    let (min_exp, max_exp) = F::exponent_limit(radix);
    let shift_exp = F::mantissa_limit(radix);
    let mantissa_size = F::MANTISSA_SIZE + 1;
    if mantissa >> mantissa_size != 0 {
        // Would require truncation of the mantissa.
        None
    } else if exponent == 0 {
        // 0 exponent, same as value, exact representation.
        let float: F = as_cast(mantissa);
        Some(float)
    } else if exponent >= min_exp && exponent <= max_exp {
        // Value can be exactly represented, return the value.
        // Use powi, since it's correct, and faster on
        // the fast-path.
        let float: F = as_cast(mantissa);
        Some(float.pow(radix, exponent))
    } else if exponent >= 0 && exponent <= max_exp + shift_exp {
        // Check to see if we have a disguised fast-path, where the
        // number of digits in the mantissa is very small, but and
        // so digits can be shifted from the exponent to the mantissa.
        // https://www.exploringbinary.com/fast-path-decimal-to-floating-point-conversion/
        let small_powers = get_small_powers_64(radix);
        let shift = exponent - max_exp;
        let power = small_powers[shift.as_usize()];

        // Compute the product of the power, if it overflows,
        // prematurely return early, otherwise, if we didn't overshoot,
        // we can get an exact value.
        let value = mantissa.checked_mul(power)?;
        if value >> mantissa_size != 0 {
            None
        } else {
            // Use powi, since it's correct, and faster on
            // the fast-path.
            let float: F = as_cast(value);
            Some(float.pow(radix, max_exp))
        }
    } else {
        // Cannot be exactly represented, exponent too small or too big,
        // would require truncation.
        None
    }
}

// POW2

// Detect if a float representation is exactly halfway after truncation.
#[cfg(feature = "radix")]
perftools_inline!{
fn is_halfway<F: FloatType>(mantissa: u64)
    -> bool
{
    // Get the leading and trailing zeros from the least-significant bit.
    let bit_length: i32 = 64 - mantissa.leading_zeros().as_i32();
    let trailing_zeros: i32 = mantissa.trailing_zeros().as_i32();

    // We need exactly mantissa+2 elements between these if it is halfway.
    // The hidden bit is mantissa+1 elements away, which is the last non-
    // truncated bit, while mantissa+2
    bit_length - trailing_zeros == F::MANTISSA_SIZE + 2
}}

// Detect if a float representation is odd after truncation.
#[cfg(feature = "radix")]
perftools_inline!{
fn is_odd<F: FloatType>(mantissa: u64)
    -> bool
{
    // Get the leading and trailing zeros from the least-significant bit.
    let bit_length: i32 = 64 - mantissa.leading_zeros().as_i32();
    let shift = bit_length - (F::MANTISSA_SIZE + 1);
    if shift >= 0 {
        // Have enough bits to have a full mantissa in the float, need to
        // check if that last bit is set.
        let mask = 1u64 << shift;
        mantissa & mask == mask
    } else {
        // Not enough bits for a full mantissa, must be even.
        false
    }
}}

/// Convert power-of-two to exact value.
///
/// We will always get an exact representation.
///
/// This works since multiplying by the exponent will not affect the
/// mantissa unless the exponent is denormal, which will cause truncation
/// regardless.
#[cfg(feature = "radix")]
fn pow2_fast_path<F>(mantissa: u64, radix: u32, pow2_exp: i32, exponent: i32)
    -> F
    where F: FloatType
{
    debug_assert!(pow2_exp != 0, "Not a power of 2.");

    // As long as the value is within the bounds, we can get an exact value.
    // Since any power of 2 only affects the exponent, we should be able to get
    // any exact value.

    // We know that if any value is > than max_exp, we get infinity, since
    // the mantissa must be positive. We know that the actual value that
    // causes underflow is 64, use 65 since that prevents inaccurate
    // rounding for any pow2_exp.
    let (min_exp, max_exp) = F::exponent_limit(radix);
    let underflow_exp = min_exp - (65 / pow2_exp);
    if exponent > max_exp {
        F::INFINITY
    } else if exponent < underflow_exp{
        F::ZERO
    } else if exponent < min_exp {
        // We know the mantissa is somewhere <= 65 below min_exp.
        // May still underflow, but it's close. Use the first multiplication
        // which guarantees no truncation, and then the second multiplication
        // which will round to the accurate representation.
        let remainder = exponent - min_exp;
        let float: F = as_cast(mantissa);
        let float = float.pow2(pow2_exp * remainder).pow2(pow2_exp * min_exp);
        float
    } else {
        let float: F = as_cast(mantissa);
        let float = float.pow2(pow2_exp * exponent);
        float
    }
}

// MODERATE
// --------

/// Multiply the floating-point by the exponent.
///
/// Multiply by pre-calculated powers of the base, modify the extended-
/// float, and return if new value and if the value can be represented
/// accurately.
fn multiply_exponent_extended<F, M>(fp: &mut ExtendedFloat<M>, radix: u32, exponent: i32, truncated: bool, kind: RoundingKind)
    -> bool
    where M: FloatErrors,
          F: FloatRounding<M>,
          ExtendedFloat<M>: ModeratePathCache<M>
{
    let powers = ExtendedFloat::<M>::get_powers(radix);
    let exponent = exponent.saturating_add(powers.bias);
    let small_index = exponent % powers.step;
    let large_index = exponent / powers.step;
    if exponent < 0 {
        // Guaranteed underflow (assign 0).
        fp.mant = M::ZERO;
        true
    } else if large_index as usize >= powers.large.len() {
        // Overflow (assign infinity)
        fp.mant = M::ONE << 63;
        fp.exp = 0x7FF;
        true
    } else {
        // Within the valid exponent range, multiply by the large and small
        // exponents and return the resulting value.

        // Track errors to as a factor of unit in last-precision.
        let mut errors: u32 = 0;
        if truncated {
            errors += M::error_halfscale();
        }

        // Multiply by the small power.
        // Check if we can directly multiply by an integer, if not,
        // use extended-precision multiplication.
        match fp.mant.overflowing_mul(powers.get_small_int(small_index.as_usize())) {
            // Overflow, multiplication unsuccessful, go slow path.
            (_, true)     => {
                fp.normalize();
                fp.imul(&powers.get_small(small_index.as_usize()));
                errors += M::error_halfscale();
            },
            // No overflow, multiplication successful.
            (mant, false) => {
                fp.mant = mant;
                fp.normalize();
            },
        }

        // Multiply by the large power
        fp.imul(&powers.get_large(large_index.as_usize()));
        if errors > 0 {
            errors += 1;
        }
        errors += M::error_halfscale();

        // Normalize the floating point (and the errors).
        let shift = fp.normalize();
        errors <<= shift;

        M::error_is_accurate::<F>(errors, &fp, kind)
    }
}

// Create a precise native float using an intermediate extended-precision float.
//
// Return the float approximation and if the value can be accurately
// represented with mantissa bits of precision.
perftools_inline_always!{
pub(super) fn moderate_path<F, M>(mantissa: M, radix: u32, exponent: i32, truncated: bool, kind: RoundingKind)
    -> (ExtendedFloat<M>, bool)
    where M: FloatErrors,
          F: FloatRounding<M> + StablePower,
          ExtendedFloat<M>: ModeratePathCache<M>
{
    let mut fp = ExtendedFloat { mant: mantissa, exp: 0 };
    let valid = multiply_exponent_extended::<F, M>(&mut fp, radix, exponent, truncated, kind);
    (fp, valid)
}}

// TO NATIVE
// ---------

// POWN

/// Fallback method. Do not inline so the stack requirements only occur
/// if required.
fn pown_fallback<'a, F, Data>(data: Data, mantissa: u64, radix: u32, lossy: bool, sign: Sign)
    -> F
    where F: FloatType,
          Data: SlowDataInterface<'a>
{
    let kind = global_rounding(sign);

    // Moderate path (use an extended 80-bit representation).
    let exponent = data.mantissa_exponent();
    let is_truncated = data.truncated_digits() != 0;
    let (fp, valid) = moderate_path::<F, _>(mantissa, radix, exponent, is_truncated, kind);
    if valid || lossy {
        let float = fp.into_rounded_float_impl::<F>(kind);
        return float;
    }

    // Slow path
    let b = fp.into_rounded_float_impl::<F>(RoundingKind::Downward);
    if b.is_special() {
        // We have a non-finite number, we get to leave early.
        return b;
    } else {
        let float = bhcomp::atof(data, radix, b, kind);
        return float;
    }
}

/// Parse non-power-of-two radix string to native float.
fn pown_to_native<'a, F, Data>(mut data: Data, bytes: &'a [u8], radix: u32, lossy: bool, sign: Sign)
    -> ParseResult<(F, *const u8)>
    where F: FloatType,
          Data: FastDataInterface<'a>
{
    // Parse the mantissa and exponent.
    let ptr = data.extract(bytes, radix)?;
    let (mantissa, truncated) = process_mantissa::<u64, _>(&data, radix);

    // Process the state to a float.
    let float = if mantissa.is_zero() {
        // Literal 0, return early.
        // Value cannot be truncated, since truncation only occurs on
        // overflow or underflow.
        F::ZERO
    } else if truncated.is_zero() {
        // Try the fast path, no mantissa truncation.
        let mant_exp = data.mantissa_exponent(0);
        if let Some(float) = fast_path::<F>(mantissa, radix, mant_exp) {
            float
        } else {
            let slow = data.to_slow(truncated);
            pown_fallback(slow, mantissa, radix, lossy, sign)
        }
    } else {
        // Can only use the moderate/slow path.
        let slow = data.to_slow(truncated);
        pown_fallback(slow, mantissa, radix, lossy, sign)
    };
    Ok((float, ptr))
}

// POW2

/// Parse power-of-two radix string to native float.
#[cfg(feature = "radix")]
fn pow2_to_native<'a, F, Data>(mut data: Data, bytes: &'a [u8], radix: u32, pow2_exp: i32, sign: Sign)
    -> ParseResult<(F, *const u8)>
    where F: FloatType,
          Data: FastDataInterface<'a>
{
    // Parse the mantissa and exponent.
    let ptr = data.extract(bytes, radix)?;
    let (mut mantissa, truncated) = process_mantissa::<u64, _>(&data, radix);

    // We have a power of 2, can get an exact value even if the mantissa
    // was truncated. Check to see if there are any truncated digits, depending
    // on our rounding scheme.
    let mantissa_size = F::MANTISSA_SIZE + 1;
    let float = if !truncated.is_zero() {
        // Truncated mantissa.
        let kind = global_rounding(sign);
        let slow = data.to_slow(truncated);
        if kind != RoundingKind::Downward {
            if cfg!(feature = "rounding") || kind == RoundingKind::NearestTieEven {
                // Need to check if we're exactly halfway and if there are truncated digits.
                if is_halfway::<F>(mantissa) && is_odd::<F>(mantissa) {
                    mantissa += 1;
                }
            } else if kind == RoundingKind::NearestTieAwayZero {
                // Need to check if we're exactly halfway and if there are truncated digits.
                if is_halfway::<F>(mantissa) {
                    mantissa += 1;
                }
            } else {
                // Need to check if there are any bytes present.
                // Check if there were any truncated bytes.
                let index = slow.mantissa_digits() - slow.truncated_digits();
                let iter = slow.integer_iter().chain(slow.fraction_iter()).skip(index);
                let count = iter.take_while(|&&c| c == b'0').count();
                let is_truncated = count < slow.truncated_digits();
                if is_truncated {
                    mantissa += 1;
                }
            }
        }

        // Create exact representation and return.
        let exponent = slow.mantissa_exponent().saturating_mul(pow2_exp);
        let fp = ExtendedFloat { mant: mantissa, exp: exponent };
        fp.into_rounded_float_impl::<F>(kind)
    } else if mantissa >> mantissa_size != 0 {
        // Would be truncated, use the extended float.
        let kind = global_rounding(sign);
        let slow = data.to_slow(truncated);
        let exponent = slow.mantissa_exponent().saturating_mul(pow2_exp);
        let fp = ExtendedFloat { mant: mantissa, exp: exponent };
        fp.into_rounded_float_impl::<F>(kind)
    } else {
        // Nothing above the hidden bit, so no rounding-error, can use the fast path.
        let mant_exp = data.mantissa_exponent(0);
        pow2_fast_path(mantissa, radix, pow2_exp, mant_exp)
    };
    Ok((float, ptr))
}

// Check if value is power of 2 and get the power.
perftools_inline!{
fn pow2_exponent(radix: u32) -> i32 {
    match radix {
        2  => 1,
        4  => 2,
        8  => 3,
        16 => 4,
        32 => 5,
        _  => 0,
    }
}}

// DISPATCHER

// Parse native float from string.
//
// The float string must be non-special, non-zero, and positive.
perftools_inline!{
fn to_native<F>(bytes: &[u8], radix: u32, lossy: bool, sign: Sign, format: NumberFormat)
    -> ParseResult<(F, *const u8)>
    where F: FloatType
{
    #[cfg(not(feature = "radix"))] {
        apply_interface!(pown_to_native, format, bytes, radix,  lossy, sign)
    }

    #[cfg(feature = "radix")] {
        let pow2_exp = pow2_exponent(radix);
        match pow2_exp {
            0 => apply_interface!(pown_to_native, format, bytes, radix, lossy, sign),
            _ => apply_interface!(pow2_to_native, format, bytes, radix, pow2_exp, sign)
        }
    }
}}

// ATOF/ATOD
// ---------

// Parse 32-bit float from string.
perftools_inline!{
pub(crate) fn atof(bytes: &[u8], radix: u32, lossy: bool, sign: Sign, format: NumberFormat)
    -> ParseResult<(f32, *const u8)>
{
    to_native::<f32>(bytes, radix, lossy, sign, format)
}}

// Parse 64-bit float from string.
perftools_inline!{
pub(crate) fn atod(bytes: &[u8], radix: u32, lossy: bool, sign: Sign, format: NumberFormat)
    -> ParseResult<(f64, *const u8)>
{
    to_native::<f64>(bytes, radix, lossy, sign, format)
}}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use crate::util::test::*;
    use super::*;

    #[test]
    fn process_mantissa_test() {
        type Data<'a> = StandardFastDataInterface<'a>;
        // 64-bits
        let data = (b!("1"), Some(b!("2345")), None, 0).into();
        assert_eq!((12345, 0), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("12"), Some(b!("345")), None, 0).into();
        assert_eq!((12345, 0), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("12345"), Some(b!("6789")), None, 0).into();
        assert_eq!((123456789, 0), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("1"), Some(b!("2345")), Some(b!("10")), 10).into();
        assert_eq!((12345, 0), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("100000000000000000000"), None, None, 0).into();
        assert_eq!((10000000000000000000, 1), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("100000000000000000001"), None, None, 0).into();
        assert_eq!((10000000000000000000, 1), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791"), Some(b!("9999999999999999999999999999999999999999999999999999999999999999999999")), None, 0).into();
        assert_eq!((17976931348623158079, 359), process_mantissa::<u64, Data>(&data, 10));

        let data = (b!("1009"), None, Some(b!("-31")), -31).into();
        assert_eq!((1009, 0), process_mantissa::<u64, Data>(&data, 10));

        // 128-bit
        let data = (b!("1"), Some(b!("2345")), None, 0).into();
        assert_eq!((12345, 0), process_mantissa::<u128, Data>(&data, 10));

        let data = (b!("12"), Some(b!("345")), None, 0).into();
        assert_eq!((12345, 0), process_mantissa::<u128, Data>(&data, 10));

        let data = (b!("12345"), Some(b!("6789")), None, 0).into();
        assert_eq!((123456789, 0), process_mantissa::<u128, Data>(&data, 10));

        let data = (b!("1"), Some(b!("2345")), Some(b!("10")), 10).into();
        assert_eq!((12345, 0), process_mantissa::<u128, Data>(&data, 10));

        let data = (b!("100000000000000000000"), None, None, 0).into();
        assert_eq!((100000000000000000000, 0), process_mantissa::<u128, Data>(&data, 10));

        let data = (b!("100000000000000000001"), None, None, 0).into();
        assert_eq!((100000000000000000001, 0), process_mantissa::<u128, Data>(&data, 10));
    }

    #[cfg(feature = "radix")]
    #[test]
    fn is_odd_test() {
        // Variant of b1000000000000000000000001, a halfway value for f32.
        assert!(is_odd::<f32>(0x1000002));
        assert!(is_odd::<f32>(0x2000004));
        assert!(is_odd::<f32>(0x8000010000000000));
        assert!(!is_odd::<f64>(0x1000002));
        assert!(!is_odd::<f64>(0x2000004));
        assert!(!is_odd::<f64>(0x8000010000000000));

        assert!(!is_odd::<f32>(0x1000001));
        assert!(!is_odd::<f32>(0x2000002));
        assert!(!is_odd::<f32>(0x8000008000000000));
        assert!(!is_odd::<f64>(0x1000001));
        assert!(!is_odd::<f64>(0x2000002));
        assert!(!is_odd::<f64>(0x8000008000000000));

        // Variant of b100000000000000000000000000000000000000000000000000001,
        // a halfway value for f64
        assert!(!is_odd::<f32>(0x3f000000000002));
        assert!(!is_odd::<f32>(0x3f000000000003));
        assert!(!is_odd::<f32>(0xFC00000000000800));
        assert!(!is_odd::<f32>(0xFC00000000000C00));
        assert!(is_odd::<f64>(0x3f000000000002));
        assert!(is_odd::<f64>(0x3f000000000003));
        assert!(is_odd::<f64>(0xFC00000000000800));
        assert!(is_odd::<f64>(0xFC00000000000C00));

        assert!(!is_odd::<f32>(0x3f000000000001));
        assert!(!is_odd::<f32>(0x3f000000000004));
        assert!(!is_odd::<f32>(0xFC00000000000400));
        assert!(!is_odd::<f32>(0xFC00000000001000));
        assert!(!is_odd::<f64>(0x3f000000000001));
        assert!(!is_odd::<f64>(0x3f000000000004));
        assert!(!is_odd::<f64>(0xFC00000000000400));
        assert!(!is_odd::<f64>(0xFC00000000001000));
    }

    #[cfg(feature = "radix")]
    #[test]
    fn is_halfway_test() {
        // Variant of b1000000000000000000000001, a halfway value for f32.
        assert!(is_halfway::<f32>(0x1000001));
        assert!(is_halfway::<f32>(0x2000002));
        assert!(is_halfway::<f32>(0x8000008000000000));
        assert!(!is_halfway::<f64>(0x1000001));
        assert!(!is_halfway::<f64>(0x2000002));
        assert!(!is_halfway::<f64>(0x8000008000000000));

        // Variant of b10000000000000000000000001, which is 1-off a halfway value.
        assert!(!is_halfway::<f32>(0x2000001));
        assert!(!is_halfway::<f64>(0x2000001));

        // Variant of b100000000000000000000000000000000000000000000000000001,
        // a halfway value for f64
        assert!(!is_halfway::<f32>(0x20000000000001));
        assert!(!is_halfway::<f32>(0x40000000000002));
        assert!(!is_halfway::<f32>(0x8000000000000400));
        assert!(is_halfway::<f64>(0x20000000000001));
        assert!(is_halfway::<f64>(0x40000000000002));
        assert!(is_halfway::<f64>(0x8000000000000400));

        // Variant of b111111000000000000000000000000000000000000000000000001,
        // a halfway value for f64.
        assert!(!is_halfway::<f32>(0x3f000000000001));
        assert!(!is_halfway::<f32>(0xFC00000000000400));
        assert!(is_halfway::<f64>(0x3f000000000001));
        assert!(is_halfway::<f64>(0xFC00000000000400));

        // Variant of b1000000000000000000000000000000000000000000000000000001,
        // which is 1-off a halfway value.
        assert!(!is_halfway::<f32>(0x40000000000001));
        assert!(!is_halfway::<f64>(0x40000000000001));
    }

    #[cfg(feature = "radix")]
    #[test]
    fn float_pow2_fast_path() {
        // Everything is valid.
        let mantissa = 1 << 63;
        for base in BASE_POW2.iter().cloned() {
            let (min_exp, max_exp) = f32::exponent_limit(base);
            let pow2_exp = pow2_exponent(base);
            for exp in min_exp-20..max_exp+30 {
                // Always valid, ignore result
                pow2_fast_path::<f32>(mantissa, base, pow2_exp, exp);
            }
        }
    }

    #[cfg(feature = "radix")]
    #[test]
    fn double_pow2_fast_path_test() {
        // Everything is valid.
        let mantissa = 1 << 63;
        for base in BASE_POW2.iter().cloned() {
            let (min_exp, max_exp) = f64::exponent_limit(base);
            let pow2_exp = pow2_exponent(base);
            for exp in min_exp-20..max_exp+30 {
                // Ignore result, always valid
                pow2_fast_path::<f64>(mantissa, base, pow2_exp, exp);
            }
        }
    }

    #[test]
    fn float_fast_path_test() {
        // valid
        let mantissa = (1 << f32::MANTISSA_SIZE) - 1;
        for base in BASE_POWN.iter().cloned() {
            let (min_exp, max_exp) = f32::exponent_limit(base);
            for exp in min_exp..max_exp+1 {
                let valid = fast_path::<f32>(mantissa, base, exp).is_some();
                assert!(valid, "should be valid {:?}.", (mantissa, base, exp));
            }
        }

        // Check slightly above valid exponents
        let f = fast_path::<f32>(123, 10, 15);
        assert_eq!(f, Some(1.23e+17));

        // Exponent is 1 too high, pushes over the mantissa.
        let f = fast_path::<f32>(123, 10, 16);
        assert!(f.is_none());

        // Mantissa is too large, checked_mul should overflow.
        let f = fast_path::<f32>(mantissa, 10, 11);
        assert!(f.is_none());

        // invalid mantissa
        #[cfg(feature = "radix")] {
            let (_, max_exp) = f64::exponent_limit(3);
            let f = fast_path::<f32>(1<<f32::MANTISSA_SIZE, 3, max_exp+1);
            assert!(f.is_none(), "invalid mantissa");
        }

        // invalid exponents
        for base in BASE_POWN.iter().cloned() {
            let (min_exp, max_exp) = f32::exponent_limit(base);
            let f = fast_path::<f32>(mantissa, base, min_exp-1);
            assert!(f.is_none(), "exponent under min_exp");

            let f = fast_path::<f32>(mantissa, base, max_exp+1);
            assert!(f.is_none(), "exponent above max_exp");
        }
    }

    #[test]
    fn double_fast_path_test() {
        // valid
        let mantissa = (1 << f64::MANTISSA_SIZE) - 1;
        for base in BASE_POWN.iter().cloned() {
            let (min_exp, max_exp) = f64::exponent_limit(base);
            for exp in min_exp..max_exp+1 {
                let f = fast_path::<f64>(mantissa, base, exp);
                assert!(f.is_some(), "should be valid {:?}.", (mantissa, base, exp));
            }
        }

        // invalid mantissa
        #[cfg(feature = "radix")] {
            let (_, max_exp) = f64::exponent_limit(3);
            let f = fast_path::<f64>(1<<f64::MANTISSA_SIZE, 3, max_exp+1);
            assert!(f.is_none(), "invalid mantissa");
        }

        // invalid exponents
        for base in BASE_POWN.iter().cloned() {
            let (min_exp, max_exp) = f64::exponent_limit(base);
            let f = fast_path::<f64>(mantissa, base, min_exp-1);
            assert!(f.is_none(), "exponent under min_exp");

            let f = fast_path::<f64>(mantissa, base, max_exp+1);
            assert!(f.is_none(), "exponent above max_exp");
        }
    }

    #[cfg(feature = "radix")]
    #[test]
    fn float_moderate_path_test() {
        // valid (overflowing small mult)
        let mantissa: u64 = 1 << 63;
        let (f, valid) = moderate_path::<f32, _>(mantissa, 3, 1, false, RoundingKind::NearestTieEven);
        assert_eq!(f.into_f32(), 2.7670116e+19);
        assert!(valid, "exponent should be valid");

        let mantissa: u64 = 4746067219335938;
        let (f, valid) = moderate_path::<f32, _>(mantissa, 15, -9, false, RoundingKind::NearestTieEven);
        assert_eq!(f.into_f32(), 123456.1);
        assert!(valid, "exponent should be valid");
    }

    #[cfg(feature = "radix")]
    #[test]
    fn double_moderate_path_test() {
        // valid (overflowing small mult)
        let mantissa: u64 = 1 << 63;
        let (f, valid) = moderate_path::<f64, _>(mantissa, 3, 1, false, RoundingKind::NearestTieEven);
        assert_eq!(f.into_f64(), 2.7670116110564327e+19);
        assert!(valid, "exponent should be valid");

        // valid (ends of the earth, salting the earth)
        let (f, valid) = moderate_path::<f64, _>(mantissa, 3, -695, true, RoundingKind::NearestTieEven);
        assert_eq!(f.into_f64(), 2.32069302345e-313);
        assert!(valid, "exponent should be valid");

        // invalid ("268A6.177777778", base 15)
        let mantissa: u64 = 4746067219335938;
        let (_, valid) = moderate_path::<f64, _>(mantissa, 15, -9, false, RoundingKind::NearestTieEven);
        assert!(!valid, "exponent should be invalid");

        // valid ("268A6.177777778", base 15)
        // 123456.10000000001300614743687445, exactly, should not round up.
        let mantissa: u128 = 4746067219335938;
        let (f, valid) = moderate_path::<f64, _>(mantissa, 15, -9, false, RoundingKind::NearestTieEven);
        assert_eq!(f.into_f64(), 123456.1);
        assert!(valid, "exponent should be valid");

        // Rounding error
        // Adapted from test-parse-random failures.
        let mantissa: u64 = 1009;
        let (_, valid) = moderate_path::<f64, _>(mantissa, 10, -31, false, RoundingKind::NearestTieEven);
        assert!(!valid, "exponent should be valid");
    }

    #[test]
    fn atof_test() {
        let atof10 = move |x| match atof(x, 10, false, Sign::Positive, NumberFormat::standard().unwrap()) {
            Ok((v, p))  => Ok((v, distance(x.as_ptr(), p))),
            Err((v, p)) => Err((v, distance(x.as_ptr(), p))),
        };

        assert_eq!(Ok((0.0, 1)), atof10(b"0"));
        assert_eq!(Ok((1.2345, 6)), atof10(b"1.2345"));
        assert_eq!(Ok((12.345, 6)), atof10(b"12.345"));
        assert_eq!(Ok((12345.6789, 10)), atof10(b"12345.6789"));
        assert_eq!(Ok((1.2345e10, 9)), atof10(b"1.2345e10"));
        assert_eq!(Ok((1.2345e-38, 10)), atof10(b"1.2345e-38"));

        // Check expected rounding, using borderline cases.
        // Round-down, halfway
        assert_eq!(Ok((16777216.0, 8)), atof10(b"16777216"));
        assert_eq!(Ok((16777216.0, 8)), atof10(b"16777217"));
        assert_eq!(Ok((16777218.0, 8)), atof10(b"16777218"));
        assert_eq!(Ok((33554432.0, 8)), atof10(b"33554432"));
        assert_eq!(Ok((33554432.0, 8)), atof10(b"33554434"));
        assert_eq!(Ok((33554436.0, 8)), atof10(b"33554436"));
        assert_eq!(Ok((17179869184.0, 11)), atof10(b"17179869184"));
        assert_eq!(Ok((17179869184.0, 11)), atof10(b"17179870208"));
        assert_eq!(Ok((17179871232.0, 11)), atof10(b"17179871232"));

        // Round-up, halfway
        assert_eq!(Ok((16777218.0, 8)), atof10(b"16777218"));
        assert_eq!(Ok((16777220.0, 8)), atof10(b"16777219"));
        assert_eq!(Ok((16777220.0, 8)), atof10(b"16777220"));
        assert_eq!(Ok((33554436.0, 8)), atof10(b"33554436"));
        assert_eq!(Ok((33554440.0, 8)), atof10(b"33554438"));
        assert_eq!(Ok((33554440.0, 8)), atof10(b"33554440"));
        assert_eq!(Ok((17179871232.0, 11)), atof10(b"17179871232"));
        assert_eq!(Ok((17179873280.0, 11)), atof10(b"17179872256"));
        assert_eq!(Ok((17179873280.0, 11)), atof10(b"17179873280"));

        // Round-up, above halfway
        assert_eq!(Ok((33554436.0, 8)), atof10(b"33554435"));
        assert_eq!(Ok((17179871232.0, 11)), atof10(b"17179870209"));

        // Check exactly halfway, round-up at halfway
        assert_eq!(Ok((1.0000001, 28)), atof10(b"1.00000017881393432617187499"));
        assert_eq!(Ok((1.0000002, 26)), atof10(b"1.000000178813934326171875"));
        assert_eq!(Ok((1.0000002, 28)), atof10(b"1.00000017881393432617187501"));

        // Invalid or partially-parsed
        assert_eq!(Err((ErrorCode::EmptyMantissa, 0)), atof10(b"e10"));
        assert_eq!(Err((ErrorCode::EmptyMantissa, 0)), atof10(b"."));
        assert_eq!(Err((ErrorCode::EmptyMantissa, 0)), atof10(b".e10"));
        assert_eq!(Err((ErrorCode::EmptyExponent, 2)), atof10(b"0e"));
        assert_eq!(Ok((1.23, 4)), atof10(b"1.23/"));
    }

    #[test]
    fn atod_test() {
        let adod_impl = move | x, r | match atod(x, r, false, Sign::Positive, NumberFormat::standard().unwrap()) {
            Ok((v, p))  => Ok((v, distance(x.as_ptr(), p))),
            Err((v, p)) => Err((v, distance(x.as_ptr(), p))),
        };
        #[cfg(feature = "radix")]
        let atod2 = move |x| adod_impl(x, 2);
        let atod10 = move |x| adod_impl(x, 10);

        assert_eq!(Ok((0.0, 1)), atod10(b"0"));
        assert_eq!(Ok((1.2345, 6)), atod10(b"1.2345"));
        assert_eq!(Ok((12.345, 6)), atod10(b"12.345"));
        assert_eq!(Ok((12345.6789, 10)), atod10(b"12345.6789"));
        assert_eq!(Ok((1.2345e10, 9)), atod10(b"1.2345e10"));
        assert_eq!(Ok((1.2345e-308, 11)), atod10(b"1.2345e-308"));

        // Check expected rounding, using borderline cases.
        // Round-down, halfway
        assert_eq!(Ok((9007199254740992.0, 16)), atod10(b"9007199254740992"));
        assert_eq!(Ok((9007199254740992.0, 16)), atod10(b"9007199254740993"));
        assert_eq!(Ok((9007199254740994.0, 16)), atod10(b"9007199254740994"));
        assert_eq!(Ok((18014398509481984.0, 17)), atod10(b"18014398509481984"));
        assert_eq!(Ok((18014398509481984.0, 17)), atod10(b"18014398509481986"));
        assert_eq!(Ok((18014398509481988.0, 17)), atod10(b"18014398509481988"));
        assert_eq!(Ok((9223372036854775808.0, 19)), atod10(b"9223372036854775808"));
        assert_eq!(Ok((9223372036854775808.0, 19)), atod10(b"9223372036854776832"));
        assert_eq!(Ok((9223372036854777856.0, 19)), atod10(b"9223372036854777856"));
        assert_eq!(Ok((11417981541647679048466287755595961091061972992.0, 47)), atod10(b"11417981541647679048466287755595961091061972992"));
        assert_eq!(Ok((11417981541647679048466287755595961091061972992.0, 47)), atod10(b"11417981541647680316116887983825362587765178368"));
        assert_eq!(Ok((11417981541647681583767488212054764084468383744.0, 47)), atod10(b"11417981541647681583767488212054764084468383744"));

        // Round-up, halfway
        assert_eq!(Ok((9007199254740994.0, 16)), atod10(b"9007199254740994"));
        assert_eq!(Ok((9007199254740996.0, 16)), atod10(b"9007199254740995"));
        assert_eq!(Ok((9007199254740996.0, 16)), atod10(b"9007199254740996"));
        assert_eq!(Ok((18014398509481988.0, 17)), atod10(b"18014398509481988"));
        assert_eq!(Ok((18014398509481992.0, 17)), atod10(b"18014398509481990"));
        assert_eq!(Ok((18014398509481992.0, 17)), atod10(b"18014398509481992"));
        assert_eq!(Ok((9223372036854777856.0, 19)), atod10(b"9223372036854777856"));
        assert_eq!(Ok((9223372036854779904.0, 19)), atod10(b"9223372036854778880"));
        assert_eq!(Ok((9223372036854779904.0, 19)), atod10(b"9223372036854779904"));
        assert_eq!(Ok((11417981541647681583767488212054764084468383744.0, 47)), atod10(b"11417981541647681583767488212054764084468383744"));
        assert_eq!(Ok((11417981541647684119068688668513567077874794496.0, 47)), atod10(b"11417981541647682851418088440284165581171589120"));
        assert_eq!(Ok((11417981541647684119068688668513567077874794496.0, 47)), atod10(b"11417981541647684119068688668513567077874794496"));

        // Round-up, above halfway
        assert_eq!(Ok((9223372036854777856.0, 19)), atod10(b"9223372036854776833"));
        assert_eq!(Ok((11417981541647681583767488212054764084468383744.0, 47)), atod10(b"11417981541647680316116887983825362587765178369"));

        // Rounding error
        // Adapted from failures in strtod.
        assert_eq!(Ok((2.2250738585072014e-308, 23)), atod10(b"2.2250738585072014e-308"));
        assert_eq!(Ok((2.225073858507201e-308, 776)), atod10(b"2.2250738585072011360574097967091319759348195463516456480234261097248222220210769455165295239081350879141491589130396211068700864386945946455276572074078206217433799881410632673292535522868813721490129811224514518898490572223072852551331557550159143974763979834118019993239625482890171070818506906306666559949382757725720157630626906633326475653000092458883164330377797918696120494973903778297049050510806099407302629371289589500035837999672072543043602840788957717961509455167482434710307026091446215722898802581825451803257070188608721131280795122334262883686223215037756666225039825343359745688844239002654981983854879482922068947216898310996983658468140228542433306603398508864458040010349339704275671864433837704860378616227717385456230658746790140867233276367187499e-308"));
        assert_eq!(Ok((2.2250738585072014e-308, 774)), atod10(b"2.22507385850720113605740979670913197593481954635164564802342610972482222202107694551652952390813508791414915891303962110687008643869459464552765720740782062174337998814106326732925355228688137214901298112245145188984905722230728525513315575501591439747639798341180199932396254828901710708185069063066665599493827577257201576306269066333264756530000924588831643303777979186961204949739037782970490505108060994073026293712895895000358379996720725430436028407889577179615094551674824347103070260914462157228988025818254518032570701886087211312807951223342628836862232150377566662250398253433597456888442390026549819838548794829220689472168983109969836584681402285424333066033985088644580400103493397042756718644338377048603786162277173854562306587467901408672332763671875e-308"));
        assert_eq!(Ok((2.2250738585072014e-308, 776)), atod10(b"2.2250738585072011360574097967091319759348195463516456480234261097248222220210769455165295239081350879141491589130396211068700864386945946455276572074078206217433799881410632673292535522868813721490129811224514518898490572223072852551331557550159143974763979834118019993239625482890171070818506906306666559949382757725720157630626906633326475653000092458883164330377797918696120494973903778297049050510806099407302629371289589500035837999672072543043602840788957717961509455167482434710307026091446215722898802581825451803257070188608721131280795122334262883686223215037756666225039825343359745688844239002654981983854879482922068947216898310996983658468140228542433306603398508864458040010349339704275671864433837704860378616227717385456230658746790140867233276367187501e-308"));
        assert_eq!(Ok((1.7976931348623157e+308, 380)), atod10(b"179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791.9999999999999999999999999999999999999999999999999999999999999999999999"));
        assert_eq!(Ok((5e-324, 761)), atod10(b"7.4109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984374999e-324"));
        assert_eq!(Ok((1e-323, 758)), atod10(b"7.4109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984375e-324"));
        assert_eq!(Ok((1e-323, 761)), atod10(b"7.4109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984375001e-324"));

        // Rounding error
        // Adapted from:
        //  https://www.exploringbinary.com/glibc-strtod-incorrectly-converts-2-to-the-negative-1075/
        #[cfg(feature = "radix")]
        assert_eq!(Ok((5e-324, 14)), atod2(b"1e-10000110010"));

        #[cfg(feature = "radix")]
        assert_eq!(Ok((0.0, 14)), atod2(b"1e-10000110011"));
        assert_eq!(Ok((0.0, 1077)), atod10(b"0.0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000024703282292062327208828439643411068618252990130716238221279284125033775363510437593264991818081799618989828234772285886546332835517796989819938739800539093906315035659515570226392290858392449105184435931802849936536152500319370457678249219365623669863658480757001585769269903706311928279558551332927834338409351978015531246597263579574622766465272827220056374006485499977096599470454020828166226237857393450736339007967761930577506740176324673600968951340535537458516661134223766678604162159680461914467291840300530057530849048765391711386591646239524912623653881879636239373280423891018672348497668235089863388587925628302755995657524455507255189313690836254779186948667994968324049705821028513185451396213837722826145437693412532098591327667236328125"));

        // Rounding error
        // Adapted from:
        //  https://www.exploringbinary.com/how-glibc-strtod-works/
        assert_eq!(Ok((2.2250738585072011e-308, 1076)), atod10(b"0.000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000022250738585072008890245868760858598876504231122409594654935248025624400092282356951787758888037591552642309780950434312085877387158357291821993020294379224223559819827501242041788969571311791082261043971979604000454897391938079198936081525613113376149842043271751033627391549782731594143828136275113838604094249464942286316695429105080201815926642134996606517803095075913058719846423906068637102005108723282784678843631944515866135041223479014792369585208321597621066375401613736583044193603714778355306682834535634005074073040135602968046375918583163124224521599262546494300836851861719422417646455137135420132217031370496583210154654068035397417906022589503023501937519773030945763173210852507299305089761582519159720757232455434770912461317493580281734466552734375"));

        // Rounding error
        // Adapted from test-parse-random failures.
        assert_eq!(Ok((1.009e-28, 8)), atod10(b"1009e-31"));
        assert_eq!(Ok((f64::INFINITY, 9)), atod10(b"18294e304"));

        // Rounding error
        // Adapted from a @dangrabcad's issue #20.
        assert_eq!(Ok((7.689539722041643e164, 21)), atod10(b"7.689539722041643e164"));
        assert_eq!(Ok((7.689539722041643e164, 165)), atod10(b"768953972204164300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"));
        assert_eq!(Ok((7.689539722041643e164, 167)), atod10(b"768953972204164300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0"));

        // Check other cases similar to @dangrabcad's issue #20.
        assert_eq!(Ok((9223372036854777856.0, 21)), atod10(b"9223372036854776833.0"));
        assert_eq!(Ok((11417981541647681583767488212054764084468383744.0, 49)), atod10(b"11417981541647680316116887983825362587765178369.0"));
        assert_eq!(Ok((9007199254740996.0, 18)), atod10(b"9007199254740995.0"));
        assert_eq!(Ok((18014398509481992.0, 19)), atod10(b"18014398509481990.0"));
        assert_eq!(Ok((9223372036854779904.0, 21)), atod10(b"9223372036854778880.0"));
        assert_eq!(Ok((11417981541647684119068688668513567077874794496.0, 49)), atod10(b"11417981541647682851418088440284165581171589120.0"));

        // Check other cases ostensibly identified via proptest.
        assert_eq!(Ok((71610528364411830000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0, 310)), atod10(b"71610528364411830000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0"));
        assert_eq!(Ok((126769393745745060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0, 311)), atod10(b"126769393745745060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0"));
        assert_eq!(Ok((38652960461239320000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0, 310)), atod10(b"38652960461239320000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0"));
    }

    #[test]
    fn atof_lossy_test() {
        let atof10 = move |x| match atof(x, 10, true, Sign::Positive, NumberFormat::standard().unwrap()) {
            Ok((v, p))  => Ok((v, distance(x.as_ptr(), p))),
            Err((v, p)) => Err((v, distance(x.as_ptr(), p))),
        };

        assert_eq!(Ok((1.2345, 6)), atof10(b"1.2345"));
        assert_eq!(Ok((12.345, 6)), atof10(b"12.345"));
        assert_eq!(Ok((12345.6789, 10)), atof10(b"12345.6789"));
        assert_eq!(Ok((1.2345e10, 9)), atof10(b"1.2345e10"));
    }

    #[test]
    fn atod_lossy_test() {
        let atod10 = move |x| match atod(x, 10, true, Sign::Positive, NumberFormat::standard().unwrap()) {
            Ok((v, p))  => Ok((v, distance(x.as_ptr(), p))),
            Err((v, p)) => Err((v, distance(x.as_ptr(), p))),
        };

        assert_eq!(Ok((1.2345, 6)), atod10(b"1.2345"));
        assert_eq!(Ok((12.345, 6)), atod10(b"12.345"));
        assert_eq!(Ok((12345.6789, 10)), atod10(b"12345.6789"));
        assert_eq!(Ok((1.2345e10, 9)), atod10(b"1.2345e10"));
    }
}
