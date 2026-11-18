//! Numeric utilities.

use easy_cast::Conv;

/// Multiplication into a wider type, which never overflows.
pub trait WideningMul {
    /// A type wide enough for the product of any two `Self`s.
    type Wide;

    /// `self * rhs`, exact.
    #[must_use]
    fn widening_mul(self, rhs: Self) -> Self::Wide;
}

/// Multiplication then division, which overflows only in the result.
pub trait SaturatingMulDiv {
    /// `self * mul / div`, rounded toward zero as `/` is, saturating at the bounds of `Self`.
    #[must_use]
    fn saturating_mul_div(self, mul: Self, div: Self) -> Self;
}

/// Implements [`WideningMul`] and [`SaturatingMulDiv`] for each `$t`, widening into `$wide`.
macro_rules! impl_mul_div {
    ($($t:ty => $wide:ty),* $(,)?) => {$(
        const _: () = assert!(size_of::<$wide>() >= 2 * size_of::<$t>());

        impl WideningMul for $t {
            type Wide = $wide;

            fn widening_mul(self, rhs: Self) -> $wide {
                <$wide>::conv(self) * <$wide>::conv(rhs)
            }
        }

        impl SaturatingMulDiv for $t {
            fn saturating_mul_div(self, mul: Self, div: Self) -> Self {
                let quotient = <Self as WideningMul>::widening_mul(self, mul) / <$wide>::conv(div);
                Self::conv(quotient.clamp(<$wide>::conv(Self::MIN), <$wide>::conv(Self::MAX)))
            }
        }
    )*};
}

/// The unsigned integer twice as wide as `usize`.
#[cfg(target_pointer_width = "16")]
type WideUsize = u32;
#[cfg(target_pointer_width = "32")]
type WideUsize = u64;
#[cfg(target_pointer_width = "64")]
type WideUsize = u128;

/// The signed integer twice as wide as `isize`.
#[cfg(target_pointer_width = "16")]
type WideIsize = i32;
#[cfg(target_pointer_width = "32")]
type WideIsize = i64;
#[cfg(target_pointer_width = "64")]
type WideIsize = i128;

impl_mul_div!(
    u8 => u16,
    u16 => u32,
    u32 => u64,
    u64 => u128,
    usize => WideUsize,
    i8 => i16,
    i16 => i32,
    i32 => i64,
    i64 => i128,
    isize => WideIsize,
);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn saturating_mul_div_is_exact_arithmetic_saturated_for_every_u8() {
        for value in u8::MIN..=u8::MAX {
            for mul in u8::MIN..=u8::MAX {
                for div in 1..=u8::MAX {
                    let exact = u32::from(value) * u32::from(mul) / u32::from(div);
                    assert_eq!(
                        u32::from(value.saturating_mul_div(mul, div)),
                        exact.min(u32::from(u8::MAX)),
                        "{value} * {mul} / {div}"
                    );
                }
            }
        }
    }

    #[rstest]
    fn saturating_mul_div_is_exact_arithmetic_saturated_for_every_i8() {
        for value in i8::MIN..=i8::MAX {
            for mul in i8::MIN..=i8::MAX {
                for div in (i8::MIN..=i8::MAX).filter(|div| *div != 0) {
                    let exact = i32::from(value) * i32::from(mul) / i32::from(div);
                    assert_eq!(
                        i32::from(value.saturating_mul_div(mul, div)),
                        exact.clamp(i32::from(i8::MIN), i32::from(i8::MAX)),
                        "{value} * {mul} / {div}"
                    );
                }
            }
        }
    }

    #[rstest]
    #[case::two_64_kib_lengths(64 * 1024, 64 * 1024, 64 * 1024 + 1, 64 * 1024 - 1)]
    #[case::product_past_usize(usize::MAX, usize::MAX, usize::MAX, usize::MAX)]
    #[case::scaled_down_from_past_usize(usize::MAX, usize::MAX / 3, usize::MAX, usize::MAX / 3)]
    #[case::saturates(usize::MAX, 2, 1, usize::MAX)]
    fn saturating_mul_div_is_exact_until_the_quotient_overflows(
        #[case] value: usize,
        #[case] mul: usize,
        #[case] div: usize,
        #[case] expected: usize,
    ) {
        assert_eq!(value.saturating_mul_div(mul, div), expected);
    }

    /// Properties of each type too wide to test exhaustively, against std's own arithmetic.
    macro_rules! properties {
        ($module:ident: $t:ty { $($widening_mul:tt)* }) => {
            mod $module {
                use easy_cast::Conv;
                use proptest::prelude::*;

                use crate::num::{SaturatingMulDiv, WideningMul};

                proptest! {
                    #[test]
                    fn saturating_mul_div_by_one_is_saturating_mul(value: $t, mul: $t) {
                        prop_assert_eq!(value.saturating_mul_div(mul, 1), value.saturating_mul(mul));
                    }

                    #[test]
                    fn saturating_mul_div_cancels_a_mul_it_divides_by(
                        value: $t,
                        mul in any::<$t>().prop_filter("a divisor", |mul| *mul != 0),
                    ) {
                        prop_assert_eq!(value.saturating_mul_div(mul, mul), value);
                    }

                    $($widening_mul)*
                }
            }
        };
    }

    macro_rules! unsigned_properties {
        ($($module:ident: $t:ty => $wide:ty),* $(,)?) => {$(
            properties!($module: $t {
                #[test]
                fn widening_mul_is_carrying_mul_without_a_carry(a: $t, b: $t) {
                    let (low, high) = a.carrying_mul(b, 0);
                    let product = <$wide>::conv(high) << <$t>::BITS | <$wide>::conv(low);
                    prop_assert_eq!(WideningMul::widening_mul(a, b), product);
                }
            });
        )*};
    }

    macro_rules! signed_properties {
        ($($module:ident: $t:ty => $wide:ty),* $(,)?) => {$(
            properties!($module: $t {
                #[test]
                fn widening_mul_is_the_product_of_the_magnitudes_signed(a: $t, b: $t) {
                    let product = WideningMul::widening_mul(a, b);
                    prop_assert_eq!(
                        product.unsigned_abs(),
                        WideningMul::widening_mul(a.unsigned_abs(), b.unsigned_abs())
                    );
                    prop_assert_eq!(product.signum(), <$wide>::conv(a.signum() * b.signum()));
                }
            });
        )*};
    }

    unsigned_properties!(
        for_u16: u16 => u32,
        for_u32: u32 => u64,
        for_u64: u64 => u128,
        for_usize: usize => crate::num::WideUsize,
    );

    signed_properties!(
        for_i16: i16 => i32,
        for_i32: i32 => i64,
        for_i64: i64 => i128,
        for_isize: isize => crate::num::WideIsize,
    );
}
