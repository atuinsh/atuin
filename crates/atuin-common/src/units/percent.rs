//! A percentage.
//!
//! This utility supports math operations, deserializing from strings and into strings.

use std::ops::Mul;
use std::str::FromStr;

use easy_cast::{ConvApprox, ConvTo, Trunc};
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// A percentage.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    PartialOrd,
    DeserializeFromStr,
    SerializeDisplay,
    derive_more::Add,
    derive_more::AddAssign,
    derive_more::Display,
    derive_more::From,
    derive_more::Into,
    derive_more::Sub,
    derive_more::SubAssign,
)]
#[display("{_0}%")]
pub struct Percent(f64);

impl Percent {
    pub const ZERO: Self = Self(0.0);
    pub const HUNDRED: Self = Self(100.0);

    #[must_use]
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.0
    }
}

/// A percentage of a percentage: `50% * 50%` is `25%`.
impl Mul for Percent {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self(self.0 * rhs.0 / 100.0)
    }
}

impl Mul<f64> for Percent {
    type Output = f64;

    fn mul(self, rhs: f64) -> f64 {
        rhs * self.0 / 100.0
    }
}

impl Mul<Percent> for f64 {
    type Output = Self;

    fn mul(self, rhs: Percent) -> Self {
        rhs * self
    }
}

impl Mul<f32> for Percent {
    type Output = f32;

    fn mul(self, rhs: f32) -> f32 {
        let share = self * f64::from(rhs);

        #[expect(clippy::cast_possible_truncation, reason = "saturation is intended")]
        let result = share as f32;

        result
    }
}

impl Mul<Percent> for f32 {
    type Output = Self;

    fn mul(self, rhs: Percent) -> Self {
        rhs * self
    }
}

/// A share of an integer, truncated toward zero and saturating at the type's bounds.
macro_rules! share_of_int {
    ($($t:ty => $to_f64:expr),* $(,)?) => {$(
        impl Mul<$t> for Percent {
            type Output = $t;

            fn mul(self, rhs: $t) -> $t {
                let share = self * $to_f64(rhs);
                <$t>::try_conv_to(Trunc, share)
                    .unwrap_or(if share < 0.0 { <$t>::MIN } else { <$t>::MAX })
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

share_of_int!(
    u8 => f64::from,
    u16 => f64::from,
    u32 => f64::from,
    i8 => f64::from,
    i16 => f64::from,
    i32 => f64::from,
    u64 => |v: u64| v as f64,
    u128 => |v: u128| v as f64,
    usize => |v: usize| v as f64,
    i64 => |v: i64| v as f64,
    i128 => |v: i128| v as f64,
    isize => |v: isize| v as f64,
);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PercentParseError {
    #[error("expected a percentage like `10%`, got an empty string")]
    Empty,
    #[error("a percentage must end with `%`")]
    MissingSign,
    #[error("`{0}` is not a number")]
    InvalidNumber(String),
    #[error("a percentage cannot be negative")]
    Negative,
}

impl FromStr for Percent {
    type Err = PercentParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(PercentParseError::Empty);
        }
        let number = s.strip_suffix('%').ok_or(PercentParseError::MissingSign)?.trim_end();
        let value: f64 = number
            .parse()
            .ok()
            .filter(|value: &f64| value.is_finite())
            .ok_or_else(|| PercentParseError::InvalidNumber(number.to_owned()))?;
        if value.is_sign_negative() {
            return Err(PercentParseError::Negative);
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::zero("0%", 0.0)]
    #[case::ten("10%", 10.0)]
    #[case::hundred("100%", 100.0)]
    #[case::above_the_whole("150%", 150.0)]
    #[case::far_above_the_whole("1000%", 1000.0)]
    #[case::fraction("2.5%", 2.5)]
    #[case::fraction_without_leading_digit(".5%", 0.5)]
    #[case::exponent("1e2%", 100.0)]
    #[case::space_before_sign("10 %", 10.0)]
    #[case::surrounding_whitespace(" 10%\n", 10.0)]
    #[case::leading_zeros("007%", 7.0)]
    fn parses_percentages(#[case] input: &str, #[case] expected: f64) {
        assert_eq!(input.parse::<Percent>().unwrap(), Percent::new(expected));
    }

    #[rstest]
    #[case::empty("", PercentParseError::Empty)]
    #[case::only_whitespace(" ", PercentParseError::Empty)]
    #[case::missing_sign("10", PercentParseError::MissingSign)]
    #[case::only_sign("%", PercentParseError::InvalidNumber("".into()))]
    #[case::word("ten%", PercentParseError::InvalidNumber("ten".into()))]
    #[case::nan("NaN%", PercentParseError::InvalidNumber("NaN".into()))]
    #[case::infinite("inf%", PercentParseError::InvalidNumber("inf".into()))]
    #[case::negative("-5%", PercentParseError::Negative)]
    #[case::negative_zero("-0%", PercentParseError::Negative)]
    fn rejects_invalid_percentages(#[case] input: &str, #[case] expected: PercentParseError) {
        assert_eq!(input.parse::<Percent>(), Err(expected));
    }

    #[rstest]
    #[case::ten_percent_of_a_thousand(10.0, 1000, 100)]
    #[case::fraction_of_a_percent(2.5, 1000, 25)]
    #[case::rounds_down(1.0, 150, 1)]
    #[case::zero_of_anything(0.0, u64::MAX, 0)]
    #[case::all_of_the_max(100.0, u64::MAX, u64::MAX)]
    #[case::half_of_the_max(50.0, u64::MAX, 1 << 63)]
    #[case::anything_of_zero(75.0, 0, 0)]
    #[case::more_than_the_whole(150.0, 1000, 1500)]
    #[case::saturates(200.0, u64::MAX, u64::MAX)]
    fn a_share_of_a_u64_works_in_either_order(
        #[case] pct: f64,
        #[case] value: u64,
        #[case] expected: u64,
    ) {
        assert_eq!(Percent::new(pct) * value, expected);
        assert_eq!(value * Percent::new(pct), expected);
    }

    #[rstest]
    fn shares_of_the_other_integer_types_saturate_at_their_own_bounds() {
        assert_eq!(Percent::new(50.0) * 200_u8, 100);
        assert_eq!(Percent::new(200.0) * 200_u8, u8::MAX);
        assert_eq!(Percent::new(50.0) * -100_i8, -50);
        assert_eq!(Percent::new(200.0) * -100_i8, i8::MIN);
        assert_eq!(Percent::new(200.0) * 100_i8, i8::MAX);
        assert_eq!(Percent::new(10.0) * 1000_usize, 100);
        assert_eq!(Percent::new(10.0) * -1000_isize, -100);
        assert_eq!(Percent::new(200.0) * u128::MAX, u128::MAX);
        assert_eq!(Percent::new(200.0) * i128::MIN, i128::MIN);
        assert_eq!(Percent::new(f64::NAN) * 10_u8, u8::MAX);
    }

    #[rstest]
    fn shares_of_floats_are_plain_float_math() {
        assert!((Percent::new(50.0) * 3.0_f64 - 1.5).abs() < f64::EPSILON);
        assert!((3.0_f64 * Percent::new(50.0) - 1.5).abs() < f64::EPSILON);
        assert!((Percent::new(250.0) * 2.0_f32 - 5.0).abs() < f32::EPSILON);
        let too_big = Percent::new(1e30) * f32::MAX;
        assert!(too_big.is_infinite() && too_big.is_sign_positive());
        let too_small = Percent::new(1e30) * f32::MIN;
        assert!(too_small.is_infinite() && too_small.is_sign_negative());
    }

    #[rstest]
    fn percentages_combine_with_each_other() {
        assert_eq!(Percent::new(50.0) * Percent::new(50.0), Percent::new(25.0));
        assert_eq!(Percent::new(10.0) + Percent::new(5.0), Percent::new(15.0));
        assert_eq!(Percent::new(10.0) - Percent::new(5.0), Percent::new(5.0));
        assert_eq!(Percent::new(10.0) - Percent::new(20.0), Percent::new(-10.0));

        let mut pct = Percent::new(10.0);
        pct += Percent::new(5.0);
        pct -= Percent::new(1.0);
        assert_eq!(pct, Percent::new(14.0));
    }

    #[rstest]
    fn constants_and_conversions() {
        assert_eq!(Percent::default(), Percent::ZERO);
        assert!((Percent::HUNDRED.value() - 100.0).abs() < f64::EPSILON);
        assert_eq!(Percent::from(42.0), Percent::new(42.0));
        assert!((f64::from(Percent::new(42.0)) - 42.0).abs() < f64::EPSILON);
    }

    #[rstest]
    #[case::whole(10.0, "10%")]
    #[case::hundred(100.0, "100%")]
    #[case::fraction(2.5, "2.5%")]
    #[case::negative(-10.0, "-10%")]
    fn displays_with_a_sign(#[case] value: f64, #[case] expected: &str) {
        assert_eq!(Percent::new(value).to_string(), expected);
    }

    #[rstest]
    fn serde_uses_the_text_form() {
        assert_eq!(serde_json::to_string(&Percent::new(2.5)).unwrap(), r#""2.5%""#);
        assert_eq!(serde_json::from_str::<Percent>(r#""2.5%""#).unwrap(), Percent::new(2.5));
        assert!(serde_json::from_str::<Percent>("10").is_err());
        assert!(serde_json::from_str::<Percent>("2.5").is_err());
    }

    proptest! {
        #[test]
        fn display_round_trips(value in 0.0..=f64::MAX) {
            let pct = Percent::new(value);
            prop_assert_eq!(pct.to_string().parse::<Percent>().unwrap(), pct);
        }

        #[test]
        fn a_share_of_at_most_the_whole_never_exceeds_it(
            pct in 0.0..=100.0_f64,
            value in 0..(1_u64 << 40),
        ) {
            prop_assert!(Percent::new(pct) * value <= value);
        }

        #[test]
        fn an_integer_share_keeps_the_sign(pct in 0.0..=1e6_f64, value in any::<i64>()) {
            let share = Percent::new(pct) * value;
            prop_assert_eq!(share < 0, value < 0 && share != 0);
        }

        #[test]
        fn a_float_share_matches_the_formula(pct in 0.0..=1e6_f64, value in -1e12..=1e12_f64) {
            let expected = value * pct / 100.0;
            prop_assert!((Percent::new(pct) * value - expected).abs() <= expected.abs() * f64::EPSILON);
        }
    }
}
