//! A whole-number percentage.

use std::num::{IntErrorKind, ParseIntError};
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};
use std::str::FromStr;

use serde_with::{DeserializeFromStr, SerializeDisplay};

/// A whole-number percentage.
///
/// The text form is `<number>%`. Nothing caps it at `100%`: `150%` of something is one and a
/// half times it. Fractions (`2.5%`) are not accepted; where finer control is needed, the config
/// takes an absolute value instead.
///
/// Multiplying a number by a `Percent` takes that share of it, in either order:
/// `Percent::new(10) * 1000_u64` and `1000_u64 * Percent::new(10)` are both `100`. This works for
/// every primitive integer and float type and for [`ByteSize`](super::ByteSize). Integer shares
/// round toward zero and saturate at the type's bounds (see the [module docs](super)).
/// Percentages also add, subtract and multiply with each other: `50% * 50%` is `25%`.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    DeserializeFromStr,
    SerializeDisplay,
    derive_more::Display,
    derive_more::From,
    derive_more::Into,
)]
#[display("{_0}%")]
pub struct Percent(u64);

impl Percent {
    pub const ZERO: Self = Self(0);
    pub const HUNDRED: Self = Self(100);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Add for Percent {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Percent {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl AddAssign for Percent {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Percent {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

/// A percentage of a percentage: `50% * 50%` is `25%`.
impl Mul for Percent {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self(self * rhs.0)
    }
}

/// `value * pct / 100`, computed so the intermediate cannot overflow even when `value` is near
/// `u128::MAX`. `None` only when the result itself does not fit.
fn scale_unsigned(value: u128, pct: u64) -> Option<u128> {
    let pct = u128::from(pct);
    (value / 100).checked_mul(pct)?.checked_add(value % 100 * pct / 100)
}

/// [`scale_unsigned`] for signed values; rounds toward zero, as integer division does.
fn scale_signed(value: i128, pct: u64) -> Option<i128> {
    let pct = i128::from(pct);
    (value / 100).checked_mul(pct)?.checked_add(value % 100 * pct / 100)
}

macro_rules! share_of_unsigned {
    ($($t:ty),*) => {$(
        impl Mul<$t> for Percent {
            type Output = $t;

            fn mul(self, rhs: $t) -> $t {
                // Widening a primitive to `u128` cannot fail; narrowing the share back can.
                u128::try_from(rhs)
                    .ok()
                    .and_then(|value| scale_unsigned(value, self.0))
                    .and_then(|share| <$t>::try_from(share).ok())
                    .unwrap_or(<$t>::MAX)
            }
        }

        impl Mul<Percent> for $t {
            type Output = $t;

            fn mul(self, rhs: Percent) -> $t {
                rhs * self
            }
        }
    )*};
}

macro_rules! share_of_signed {
    ($($t:ty),*) => {$(
        impl Mul<$t> for Percent {
            type Output = $t;

            fn mul(self, rhs: $t) -> $t {
                let bound = if rhs < 0 { <$t>::MIN } else { <$t>::MAX };
                i128::try_from(rhs)
                    .ok()
                    .and_then(|value| scale_signed(value, self.0))
                    .and_then(|share| <$t>::try_from(share).ok())
                    .unwrap_or(bound)
            }
        }

        impl Mul<Percent> for $t {
            type Output = $t;

            fn mul(self, rhs: Percent) -> $t {
                rhs * self
            }
        }
    )*};
}

macro_rules! share_of_float {
    ($($t:ty),*) => {$(
        impl Mul<$t> for Percent {
            type Output = $t;

            fn mul(self, rhs: $t) -> $t {
                rhs * (self.0 as $t) / 100.0
            }
        }

        impl Mul<Percent> for $t {
            type Output = $t;

            fn mul(self, rhs: Percent) -> $t {
                rhs * self
            }
        }
    )*};
}

share_of_unsigned!(u8, u16, u32, u64, u128, usize);
share_of_signed!(i8, i16, i32, i64, i128, isize);
share_of_float!(f32, f64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PercentParseError {
    #[error("expected a percentage like `10%`, got an empty string")]
    Empty,
    #[error("a percentage must end with `%`")]
    MissingSign,
    #[error("`{0}` is not a whole number")]
    InvalidNumber(String),
    #[error("percentage is too large to represent")]
    Overflow,
}

impl FromStr for Percent {
    type Err = PercentParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(PercentParseError::Empty);
        }
        let number = s.strip_suffix('%').ok_or(PercentParseError::MissingSign)?.trim_end();
        number.parse().map(Self).map_err(|e: ParseIntError| match e.kind() {
            IntErrorKind::PosOverflow => PercentParseError::Overflow,
            _ => PercentParseError::InvalidNumber(number.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::zero("0%", 0)]
    #[case::ten("10%", 10)]
    #[case::hundred("100%", 100)]
    #[case::above_the_whole("150%", 150)]
    #[case::far_above_the_whole("1000%", 1000)]
    #[case::space_before_sign("10 %", 10)]
    #[case::surrounding_whitespace(" 10%\n", 10)]
    #[case::leading_zeros("007%", 7)]
    #[case::max("18446744073709551615%", u64::MAX)]
    fn parses_whole_percentages(#[case] input: &str, #[case] expected: u64) {
        assert_eq!(input.parse::<Percent>().unwrap(), Percent::new(expected));
    }

    #[rstest]
    #[case::empty("", PercentParseError::Empty)]
    #[case::only_whitespace(" ", PercentParseError::Empty)]
    #[case::missing_sign("10", PercentParseError::MissingSign)]
    #[case::only_sign("%", PercentParseError::InvalidNumber("".into()))]
    #[case::fraction("2.5%", PercentParseError::InvalidNumber("2.5".into()))]
    #[case::negative("-5%", PercentParseError::InvalidNumber("-5".into()))]
    #[case::overflow("18446744073709551616%", PercentParseError::Overflow)]
    fn rejects_invalid_percentages(#[case] input: &str, #[case] expected: PercentParseError) {
        assert_eq!(input.parse::<Percent>(), Err(expected));
    }

    #[rstest]
    #[case::ten_percent_of_a_thousand(10, 1000, 100)]
    #[case::rounds_down(1, 150, 1)]
    #[case::zero_of_anything(0, u64::MAX, 0)]
    #[case::all_of_the_max(100, u64::MAX, u64::MAX)]
    #[case::half_of_the_max(50, u64::MAX, u64::MAX / 2)]
    #[case::anything_of_zero(75, 0, 0)]
    #[case::more_than_the_whole(150, 1000, 1500)]
    #[case::saturates(200, u64::MAX, u64::MAX)]
    fn a_share_of_a_u64_works_in_either_order(
        #[case] pct: u64,
        #[case] value: u64,
        #[case] expected: u64,
    ) {
        assert_eq!(Percent::new(pct) * value, expected);
        assert_eq!(value * Percent::new(pct), expected);
    }

    #[rstest]
    fn shares_of_the_other_integer_types_saturate_at_their_own_bounds() {
        assert_eq!(Percent::new(50) * 200_u8, 100);
        assert_eq!(Percent::new(200) * 200_u8, u8::MAX);
        assert_eq!(Percent::new(50) * -100_i8, -50);
        assert_eq!(Percent::new(200) * -100_i8, i8::MIN);
        assert_eq!(Percent::new(200) * 100_i8, i8::MAX);
        assert_eq!(Percent::new(10) * usize::MAX, usize::MAX / 10);
        assert_eq!(Percent::new(10) * -1000_isize, -100);
    }

    /// The point of splitting the multiplication: a value near the top of the widest type must
    /// not lose its share to an overflowing intermediate.
    #[rstest]
    fn shares_of_the_widest_types_are_exact() {
        assert_eq!(Percent::new(50) * u128::MAX, u128::MAX / 2);
        assert_eq!(Percent::new(100) * u128::MAX, u128::MAX);
        assert_eq!(Percent::new(101) * u128::MAX, u128::MAX);
        assert_eq!(Percent::new(50) * i128::MIN, i128::MIN / 2);
        assert_eq!(Percent::new(200) * i128::MIN, i128::MIN);
    }

    #[rstest]
    fn shares_of_floats_are_plain_float_math() {
        assert!((Percent::new(50) * 3.0_f64 - 1.5).abs() < f64::EPSILON);
        assert!((3.0_f64 * Percent::new(50) - 1.5).abs() < f64::EPSILON);
        assert!((Percent::new(250) * 2.0_f32 - 5.0).abs() < f32::EPSILON);
    }

    #[rstest]
    fn percentages_combine_with_each_other() {
        assert_eq!(Percent::new(50) * Percent::new(50), Percent::new(25));
        assert_eq!(Percent::new(10) + Percent::new(5), Percent::new(15));
        assert_eq!(Percent::new(10) - Percent::new(5), Percent::new(5));
        // saturating, like every other operation here
        assert_eq!(Percent::new(10) - Percent::new(20), Percent::ZERO);
        assert_eq!(Percent::new(u64::MAX) + Percent::new(1), Percent::new(u64::MAX));

        let mut pct = Percent::new(10);
        pct += Percent::new(5);
        pct -= Percent::new(1);
        assert_eq!(pct, Percent::new(14));
    }

    #[rstest]
    fn constants_and_conversions() {
        assert_eq!(Percent::default(), Percent::ZERO);
        assert_eq!(Percent::HUNDRED.value(), 100);
        assert_eq!(Percent::from(42), Percent::new(42));
        assert_eq!(u64::from(Percent::new(42)), 42);
    }

    #[rstest]
    fn displays_with_a_sign() {
        assert_eq!(Percent::new(10).to_string(), "10%");
        assert_eq!(Percent::HUNDRED.to_string(), "100%");
    }

    #[rstest]
    fn serde_uses_the_text_form() {
        assert_eq!(serde_json::to_string(&Percent::new(10)).unwrap(), r#""10%""#);
        assert_eq!(serde_json::from_str::<Percent>(r#""10%""#).unwrap(), Percent::new(10));
        assert!(serde_json::from_str::<Percent>("10").is_err());
    }

    proptest! {
        #[test]
        fn display_round_trips(value in any::<u64>()) {
            let pct = Percent::new(value);
            prop_assert_eq!(pct.to_string().parse::<Percent>().unwrap(), pct);
            prop_assert_eq!(pct.value(), value);
        }

        /// The share agrees with the obvious wide-arithmetic definition, and is commutative.
        #[test]
        fn a_u64_share_matches_the_reference(pct in any::<u64>(), value in any::<u64>()) {
            let reference = u128::from(value) * u128::from(pct) / 100;
            let expected = u64::try_from(reference).unwrap_or(u64::MAX);
            prop_assert_eq!(Percent::new(pct) * value, expected);
            prop_assert_eq!(value * Percent::new(pct), expected);
        }

        #[test]
        fn a_share_of_at_most_the_whole_never_exceeds_it(
            pct in 0..=100_u64,
            value in any::<u64>(),
        ) {
            prop_assert!(Percent::new(pct) * value <= value);
        }

        #[test]
        fn a_signed_share_keeps_the_sign(pct in any::<u64>(), value in any::<i64>()) {
            let share = Percent::new(pct) * value;
            prop_assert_eq!(share.signum() == -1, value < 0 && share != 0);
        }
    }
}
