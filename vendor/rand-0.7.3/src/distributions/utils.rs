// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Math helper functions

#[cfg(feature = "std")] use crate::distributions::ziggurat_tables;
#[cfg(feature = "std")] use crate::Rng;
#[cfg(feature = "simd_support")] use packed_simd::*;


pub trait WideningMultiply<RHS = Self> {
    type Output;

    fn wmul(self, x: RHS) -> Self::Output;
}

macro_rules! wmul_impl {
    ($ty:ty, $wide:ty, $shift:expr) => {
        impl WideningMultiply for $ty {
            type Output = ($ty, $ty);

            #[inline(always)]
            fn wmul(self, x: $ty) -> Self::Output {
                let tmp = (self as $wide) * (x as $wide);
                ((tmp >> $shift) as $ty, tmp as $ty)
            }
        }
    };

    // simd bulk implementation
    ($(($ty:ident, $wide:ident),)+, $shift:expr) => {
        $(
            impl WideningMultiply for $ty {
                type Output = ($ty, $ty);

                #[inline(always)]
                fn wmul(self, x: $ty) -> Self::Output {
                    // For supported vectors, this should compile to a couple
                    // supported multiply & swizzle instructions (no actual
                    // casting).
                    // TODO: optimize
                    let y: $wide = self.cast();
                    let x: $wide = x.cast();
                    let tmp = y * x;
                    let hi: $ty = (tmp >> $shift).cast();
                    let lo: $ty = tmp.cast();
                    (hi, lo)
                }
            }
        )+
    };
}
wmul_impl! { u8, u16, 8 }
wmul_impl! { u16, u32, 16 }
wmul_impl! { u32, u64, 32 }
#[cfg(not(target_os = "emscripten"))]
wmul_impl! { u64, u128, 64 }

// This code is a translation of the __mulddi3 function in LLVM's
// compiler-rt. It is an optimised variant of the common method
// `(a + b) * (c + d) = ac + ad + bc + bd`.
//
// For some reason LLVM can optimise the C version very well, but
// keeps shuffling registers in this Rust translation.
macro_rules! wmul_impl_large {
    ($ty:ty, $half:expr) => {
        impl WideningMultiply for $ty {
            type Output = ($ty, $ty);

            #[inline(always)]
            fn wmul(self, b: $ty) -> Self::Output {
                const LOWER_MASK: $ty = !0 >> $half;
                let mut low = (self & LOWER_MASK).wrapping_mul(b & LOWER_MASK);
                let mut t = low >> $half;
                low &= LOWER_MASK;
                t += (self >> $half).wrapping_mul(b & LOWER_MASK);
                low += (t & LOWER_MASK) << $half;
                let mut high = t >> $half;
                t = low >> $half;
                low &= LOWER_MASK;
                t += (b >> $half).wrapping_mul(self & LOWER_MASK);
                low += (t & LOWER_MASK) << $half;
                high += t >> $half;
                high += (self >> $half).wrapping_mul(b >> $half);

                (high, low)
            }
        }
    };

    // simd bulk implementation
    (($($ty:ty,)+) $scalar:ty, $half:expr) => {
        $(
            impl WideningMultiply for $ty {
                type Output = ($ty, $ty);

                #[inline(always)]
                fn wmul(self, b: $ty) -> Self::Output {
                    // needs wrapping multiplication
                    const LOWER_MASK: $scalar = !0 >> $half;
                    let mut low = (self & LOWER_MASK) * (b & LOWER_MASK);
                    let mut t = low >> $half;
                    low &= LOWER_MASK;
                    t += (self >> $half) * (b & LOWER_MASK);
                    low += (t & LOWER_MASK) << $half;
                    let mut high = t >> $half;
                    t = low >> $half;
                    low &= LOWER_MASK;
                    t += (b >> $half) * (self & LOWER_MASK);
                    low += (t & LOWER_MASK) << $half;
                    high += t >> $half;
                    high += (self >> $half) * (b >> $half);

                    (high, low)
                }
            }
        )+
    };
}
#[cfg(target_os = "emscripten")]
wmul_impl_large! { u64, 32 }
#[cfg(not(target_os = "emscripten"))]
wmul_impl_large! { u128, 64 }

macro_rules! wmul_impl_usize {
    ($ty:ty) => {
        impl WideningMultiply for usize {
            type Output = (usize, usize);

            #[inline(always)]
            fn wmul(self, x: usize) -> Self::Output {
                let (high, low) = (self as $ty).wmul(x as $ty);
                (high as usize, low as usize)
            }
        }
    };
}
#[cfg(target_pointer_width = "32")]
wmul_impl_usize! { u32 }
#[cfg(target_pointer_width = "64")]
wmul_impl_usize! { u64 }

#[cfg(all(feature = "simd_support", feature = "nightly"))]
mod simd_wmul {
    use super::*;
    #[cfg(target_arch = "x86")] use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")] use core::arch::x86_64::*;

    wmul_impl! {
        (u8x2, u16x2),
        (u8x4, u16x4),
        (u8x8, u16x8),
        (u8x16, u16x16),
        (u8x32, u16x32),,
        8
    }

    wmul_impl! { (u16x2, u32x2),, 16 }
    #[cfg(not(target_feature = "sse2"))]
    wmul_impl! { (u16x4, u32x4),, 16 }
    #[cfg(not(target_feature = "sse4.2"))]
    wmul_impl! { (u16x8, u32x8),, 16 }
    #[cfg(not(target_feature = "avx2"))]
    wmul_impl! { (u16x16, u32x16),, 16 }

    // 16-bit lane widths allow use of the x86 `mulhi` instructions, which
    // means `wmul` can be implemented with only two instructions.
    #[allow(unused_macros)]
    macro_rules! wmul_impl_16 {
        ($ty:ident, $intrinsic:ident, $mulhi:ident, $mullo:ident) => {
            impl WideningMultiply for $ty {
                type Output = ($ty, $ty);

                #[inline(always)]
                fn wmul(self, x: $ty) -> Self::Output {
                    let b = $intrinsic::from_bits(x);
                    let a = $intrinsic::from_bits(self);
                    let hi = $ty::from_bits(unsafe { $mulhi(a, b) });
                    let lo = $ty::from_bits(unsafe { $mullo(a, b) });
                    (hi, lo)
                }
            }
        };
    }

    #[cfg(target_feature = "sse2")]
    wmul_impl_16! { u16x4, __m64, _mm_mulhi_pu16, _mm_mullo_pi16 }
    #[cfg(target_feature = "sse4.2")]
    wmul_impl_16! { u16x8, __m128i, _mm_mulhi_epu16, _mm_mullo_epi16 }
    #[cfg(target_feature = "avx2")]
    wmul_impl_16! { u16x16, __m256i, _mm256_mulhi_epu16, _mm256_mullo_epi16 }
    // FIXME: there are no `__m512i` types in stdsimd yet, so `wmul::<u16x32>`
    // cannot use the same implementation.

    wmul_impl! {
        (u32x2, u64x2),
        (u32x4, u64x4),
        (u32x8, u64x8),,
        32
    }

    // TODO: optimize, this seems to seriously slow things down
    wmul_impl_large! { (u8x64,) u8, 4 }
    wmul_impl_large! { (u16x32,) u16, 8 }
    wmul_impl_large! { (u32x16,) u32, 16 }
    wmul_impl_large! { (u64x2, u64x4, u64x8,) u64, 32 }
}
#[cfg(all(feature = "simd_support", feature = "nightly"))]
pub use self::simd_wmul::*;


/// Helper trait when dealing with scalar and SIMD floating point types.
pub(crate) trait FloatSIMDUtils {
    // `PartialOrd` for vectors compares lexicographically. We want to compare all
    // the individual SIMD lanes instead, and get the combined result over all
    // lanes. This is possible using something like `a.lt(b).all()`, but we
    // implement it as a trait so we can write the same code for `f32` and `f64`.
    // Only the comparison functions we need are implemented.
    fn all_lt(self, other: Self) -> bool;
    fn all_le(self, other: Self) -> bool;
    fn all_finite(self) -> bool;

    type Mask;
    fn finite_mask(self) -> Self::Mask;
    fn gt_mask(self, other: Self) -> Self::Mask;
    fn ge_mask(self, other: Self) -> Self::Mask;

    // Decrease all lanes where the mask is `true` to the next lower value
    // representable by the floating-point type. At least one of the lanes
    // must be set.
    fn decrease_masked(self, mask: Self::Mask) -> Self;

    // Convert from int value. Conversion is done while retaining the numerical
    // value, not by retaining the binary representation.
    type UInt;
    fn cast_from_int(i: Self::UInt) -> Self;
}

/// Implement functions available in std builds but missing from core primitives
#[cfg(not(std))]
pub(crate) trait Float: Sized {
    fn is_nan(self) -> bool;
    fn is_infinite(self) -> bool;
    fn is_finite(self) -> bool;
}

/// Implement functions on f32/f64 to give them APIs similar to SIMD types
pub(crate) trait FloatAsSIMD: Sized {
    #[inline(always)]
    fn lanes() -> usize {
        1
    }
    #[inline(always)]
    fn splat(scalar: Self) -> Self {
        scalar
    }
    #[inline(always)]
    fn extract(self, index: usize) -> Self {
        debug_assert_eq!(index, 0);
        self
    }
    #[inline(always)]
    fn replace(self, index: usize, new_value: Self) -> Self {
        debug_assert_eq!(index, 0);
        new_value
    }
}

pub(crate) trait BoolAsSIMD: Sized {
    fn any(self) -> bool;
    fn all(self) -> bool;
    fn none(self) -> bool;
}

impl BoolAsSIMD for bool {
    #[inline(always)]
    fn any(self) -> bool {
        self
    }

    #[inline(always)]
    fn all(self) -> bool {
        self
    }

    #[inline(always)]
    fn none(self) -> bool {
        !self
    }
}

macro_rules! scalar_float_impl {
    ($ty:ident, $uty:ident) => {
        #[cfg(not(std))]
        impl Float for $ty {
            #[inline]
            fn is_nan(self) -> bool {
                self != self
            }

            #[inline]
            fn is_infinite(self) -> bool {
                self == ::core::$ty::INFINITY || self == ::core::$ty::NEG_INFINITY
            }

            #[inline]
            fn is_finite(self) -> bool {
                !(self.is_nan() || self.is_infinite())
            }
        }

        impl FloatSIMDUtils for $ty {
            type Mask = bool;
            type UInt = $uty;

            #[inline(always)]
            fn all_lt(self, other: Self) -> bool {
                self < other
            }

            #[inline(always)]
            fn all_le(self, other: Self) -> bool {
                self <= other
            }

            #[inline(always)]
            fn all_finite(self) -> bool {
                self.is_finite()
            }

            #[inline(always)]
            fn finite_mask(self) -> Self::Mask {
                self.is_finite()
            }

            #[inline(always)]
            fn gt_mask(self, other: Self) -> Self::Mask {
                self > other
            }

            #[inline(always)]
            fn ge_mask(self, other: Self) -> Self::Mask {
                self >= other
            }

            #[inline(always)]
            fn decrease_masked(self, mask: Self::Mask) -> Self {
                debug_assert!(mask, "At least one lane must be set");
                <$ty>::from_bits(self.to_bits() - 1)
            }

            #[inline]
            fn cast_from_int(i: Self::UInt) -> Self {
                i as $ty
            }
        }

        impl FloatAsSIMD for $ty {}
    };
}

scalar_float_impl!(f32, u32);
scalar_float_impl!(f64, u64);


#[cfg(feature = "simd_support")]
macro_rules! simd_impl {
    ($ty:ident, $f_scalar:ident, $mty:ident, $uty:ident) => {
        impl FloatSIMDUtils for $ty {
            type Mask = $mty;
            type UInt = $uty;

            #[inline(always)]
            fn all_lt(self, other: Self) -> bool {
                self.lt(other).all()
            }

            #[inline(always)]
            fn all_le(self, other: Self) -> bool {
                self.le(other).all()
            }

            #[inline(always)]
            fn all_finite(self) -> bool {
                self.finite_mask().all()
            }

            #[inline(always)]
            fn finite_mask(self) -> Self::Mask {
                // This can possibly be done faster by checking bit patterns
                let neg_inf = $ty::splat(::core::$f_scalar::NEG_INFINITY);
                let pos_inf = $ty::splat(::core::$f_scalar::INFINITY);
                self.gt(neg_inf) & self.lt(pos_inf)
            }

            #[inline(always)]
            fn gt_mask(self, other: Self) -> Self::Mask {
                self.gt(other)
            }

            #[inline(always)]
            fn ge_mask(self, other: Self) -> Self::Mask {
                self.ge(other)
            }

            #[inline(always)]
            fn decrease_masked(self, mask: Self::Mask) -> Self {
                // Casting a mask into ints will produce all bits set for
                // true, and 0 for false. Adding that to the binary
                // representation of a float means subtracting one from
                // the binary representation, resulting in the next lower
                // value representable by $ty. This works even when the
                // current value is infinity.
                debug_assert!(mask.any(), "At least one lane must be set");
                <$ty>::from_bits(<$uty>::from_bits(self) + <$uty>::from_bits(mask))
            }

            #[inline]
            fn cast_from_int(i: Self::UInt) -> Self {
                i.cast()
            }
        }
    };
}

#[cfg(feature="simd_support")] simd_impl! { f32x2, f32, m32x2, u32x2 }
#[cfg(feature="simd_support")] simd_impl! { f32x4, f32, m32x4, u32x4 }
#[cfg(feature="simd_support")] simd_impl! { f32x8, f32, m32x8, u32x8 }
#[cfg(feature="simd_support")] simd_impl! { f32x16, f32, m32x16, u32x16 }
#[cfg(feature="simd_support")] simd_impl! { f64x2, f64, m64x2, u64x2 }
#[cfg(feature="simd_support")] simd_impl! { f64x4, f64, m64x4, u64x4 }
#[cfg(feature="simd_support")] simd_impl! { f64x8, f64, m64x8, u64x8 }

/// Calculates ln(gamma(x)) (natural logarithm of the gamma
/// function) using the Lanczos approximation.
///
/// The approximation expresses the gamma function as:
/// `gamma(z+1) = sqrt(2*pi)*(z+g+0.5)^(z+0.5)*exp(-z-g-0.5)*Ag(z)`
/// `g` is an arbitrary constant; we use the approximation with `g=5`.
///
/// Noting that `gamma(z+1) = z*gamma(z)` and applying `ln` to both sides:
/// `ln(gamma(z)) = (z+0.5)*ln(z+g+0.5)-(z+g+0.5) + ln(sqrt(2*pi)*Ag(z)/z)`
///
/// `Ag(z)` is an infinite series with coefficients that can be calculated
/// ahead of time - we use just the first 6 terms, which is good enough
/// for most purposes.
#[cfg(feature = "std")]
pub fn log_gamma(x: f64) -> f64 {
    // precalculated 6 coefficients for the first 6 terms of the series
    let coefficients: [f64; 6] = [
        76.18009172947146,
        -86.50532032941677,
        24.01409824083091,
        -1.231739572450155,
        0.1208650973866179e-2,
        -0.5395239384953e-5,
    ];

    // (x+0.5)*ln(x+g+0.5)-(x+g+0.5)
    let tmp = x + 5.5;
    let log = (x + 0.5) * tmp.ln() - tmp;

    // the first few terms of the series for Ag(x)
    let mut a = 1.000000000190015;
    let mut denom = x;
    for coeff in &coefficients {
        denom += 1.0;
        a += coeff / denom;
    }

    // get everything together
    // a is Ag(x)
    // 2.5066... is sqrt(2pi)
    log + (2.5066282746310005 * a / x).ln()
}

/// Sample a random number using the Ziggurat method (specifically the
/// ZIGNOR variant from Doornik 2005). Most of the arguments are
/// directly from the paper:
///
/// * `rng`: source of randomness
/// * `symmetric`: whether this is a symmetric distribution, or one-sided with P(x < 0) = 0.
/// * `X`: the $x_i$ abscissae.
/// * `F`: precomputed values of the PDF at the $x_i$, (i.e. $f(x_i)$)
/// * `F_DIFF`: precomputed values of $f(x_i) - f(x_{i+1})$
/// * `pdf`: the probability density function
/// * `zero_case`: manual sampling from the tail when we chose the
///    bottom box (i.e. i == 0)

// the perf improvement (25-50%) is definitely worth the extra code
// size from force-inlining.
#[cfg(feature = "std")]
#[inline(always)]
pub fn ziggurat<R: Rng + ?Sized, P, Z>(
    rng: &mut R,
    symmetric: bool,
    x_tab: ziggurat_tables::ZigTable,
    f_tab: ziggurat_tables::ZigTable,
    mut pdf: P,
    mut zero_case: Z
) -> f64
where
    P: FnMut(f64) -> f64,
    Z: FnMut(&mut R, f64) -> f64,
{
    use crate::distributions::float::IntoFloat;
    loop {
        // As an optimisation we re-implement the conversion to a f64.
        // From the remaining 12 most significant bits we use 8 to construct `i`.
        // This saves us generating a whole extra random number, while the added
        // precision of using 64 bits for f64 does not buy us much.
        let bits = rng.next_u64();
        let i = bits as usize & 0xff;

        let u = if symmetric {
            // Convert to a value in the range [2,4) and substract to get [-1,1)
            // We can't convert to an open range directly, that would require
            // substracting `3.0 - EPSILON`, which is not representable.
            // It is possible with an extra step, but an open range does not
            // seem neccesary for the ziggurat algorithm anyway.
            (bits >> 12).into_float_with_exponent(1) - 3.0
        } else {
            // Convert to a value in the range [1,2) and substract to get (0,1)
            (bits >> 12).into_float_with_exponent(0) - (1.0 - ::core::f64::EPSILON / 2.0)
        };
        let x = u * x_tab[i];

        let test_x = if symmetric { x.abs() } else { x };

        // algebraically equivalent to |u| < x_tab[i+1]/x_tab[i] (or u < x_tab[i+1]/x_tab[i])
        if test_x < x_tab[i + 1] {
            return x;
        }
        if i == 0 {
            return zero_case(rng, u);
        }
        // algebraically equivalent to f1 + DRanU()*(f0 - f1) < 1
        if f_tab[i + 1] + (f_tab[i] - f_tab[i + 1]) * rng.gen::<f64>() < pdf(x) {
            return x;
        }
    }
}
