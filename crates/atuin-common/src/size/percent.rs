//! A whole-number percentage.

use std::str::FromStr;

use serde_with::{DeserializeFromStr, SerializeDisplay};

use super::ByteSize;

/// A whole-number percentage, `0%` to `100%` inclusive.
///
/// The text form is `<number>%`. Fractions (`2.5%`) are not accepted: where finer control is
/// needed, the config takes an absolute [`ByteSize`] instead.
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
    derive_more::Into,
)]
#[display("{_0}%")]
pub struct Percent(u8);

impl Percent {
    pub const ZERO: Self = Self(0);
    pub const FULL: Self = Self(100);

    /// `None` if `value` is more than 100.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 100 {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }

    /// This share of `total`, rounded down to a whole byte.
    #[must_use]
    pub fn of(self, total: ByteSize) -> ByteSize {
        let bytes = u128::from(total.bytes()) * u128::from(self.0) / 100;
        ByteSize::from_bytes(u64::try_from(bytes).expect("at most 100% of a u64 fits in a u64"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PercentParseError {
    #[error("expected a percentage like `10%`, got an empty string")]
    Empty,
    #[error("a percentage must end with `%`")]
    MissingSign,
    #[error("`{0}` is not a whole number")]
    InvalidNumber(String),
    #[error("{0}% is more than 100%")]
    OutOfRange(u64),
}

impl FromStr for Percent {
    type Err = PercentParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(PercentParseError::Empty);
        }
        let number = s.strip_suffix('%').ok_or(PercentParseError::MissingSign)?.trim_end();
        let value: u64 =
            number.parse().map_err(|_| PercentParseError::InvalidNumber(number.to_owned()))?;
        u8::try_from(value).ok().and_then(Self::new).ok_or(PercentParseError::OutOfRange(value))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn percent(value: u8) -> Percent {
        Percent::new(value).expect("test percentages are within range")
    }

    #[rstest]
    #[case::zero("0%", 0)]
    #[case::ten("10%", 10)]
    #[case::full("100%", 100)]
    #[case::space_before_sign("10 %", 10)]
    #[case::surrounding_whitespace(" 10%\n", 10)]
    #[case::leading_zeros("007%", 7)]
    fn parses_whole_percentages(#[case] input: &str, #[case] expected: u8) {
        assert_eq!(input.parse::<Percent>().unwrap(), percent(expected));
    }

    #[rstest]
    #[case::empty("", PercentParseError::Empty)]
    #[case::only_whitespace(" ", PercentParseError::Empty)]
    #[case::missing_sign("10", PercentParseError::MissingSign)]
    #[case::only_sign("%", PercentParseError::InvalidNumber("".into()))]
    #[case::fraction("2.5%", PercentParseError::InvalidNumber("2.5".into()))]
    #[case::negative("-5%", PercentParseError::InvalidNumber("-5".into()))]
    #[case::huge(
        "99999999999999999999%",
        PercentParseError::InvalidNumber("99999999999999999999".into())
    )]
    #[case::over_100("150%", PercentParseError::OutOfRange(150))]
    #[case::just_over_100("101%", PercentParseError::OutOfRange(101))]
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
    fn of_takes_a_share_of_a_size(#[case] pct: u8, #[case] total: u64, #[case] expected: u64) {
        assert_eq!(percent(pct).of(ByteSize::from_bytes(total)), ByteSize::from_bytes(expected));
    }

    #[rstest]
    fn new_rejects_more_than_100() {
        assert_eq!(Percent::new(101), None);
        assert_eq!(Percent::new(255), None);
        assert_eq!(Percent::new(100), Some(Percent::FULL));
        assert_eq!(Percent::new(0), Some(Percent::ZERO));
        assert_eq!(Percent::default(), Percent::ZERO);
    }

    #[rstest]
    fn displays_with_a_sign() {
        assert_eq!(percent(10).to_string(), "10%");
        assert_eq!(Percent::FULL.to_string(), "100%");
    }

    #[rstest]
    fn serde_uses_the_text_form() {
        assert_eq!(serde_json::to_string(&percent(10)).unwrap(), r#""10%""#);
        assert_eq!(serde_json::from_str::<Percent>(r#""10%""#).unwrap(), percent(10));
        assert!(serde_json::from_str::<Percent>("10").is_err());
        assert!(serde_json::from_str::<Percent>(r#""150%""#).is_err());
    }

    proptest! {
        #[test]
        fn display_round_trips(value in 0..=100u8) {
            let pct = percent(value);
            prop_assert_eq!(pct.to_string().parse::<Percent>().unwrap(), pct);
            prop_assert_eq!(u8::from(pct), value);
            prop_assert_eq!(pct.value(), value);
        }

        #[test]
        fn of_never_exceeds_the_total(value in 0..=100u8, total in any::<u64>()) {
            let share = percent(value).of(ByteSize::from_bytes(total));
            prop_assert!(share.bytes() <= total);
        }
    }
}
