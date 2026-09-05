//! A count of bytes with a human-friendly text form.

use std::fmt;
use std::iter::Sum;
use std::num::{IntErrorKind, ParseIntError};
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};
use std::str::FromStr;

use serde::{Deserialize, Deserializer};
use serde_with::SerializeDisplay;

use super::{Percent, text_or_bytes};

/// A number of bytes.
///
/// The text form is `<number><unit>`: `1MB`, `512 KB`, `1.5GB`, or a bare `4096` for bytes.
/// Units are powers of 1024 -- `1MB` is 1,048,576 bytes. Fractions of a unit are floored to whole
/// bytes; fractions of a *byte* are rejected.
///
/// Serializes as its text form. Deserializes from either the text form or a bare integer, so
/// `max_output_size = "1MB"` and `max_output_size = 1048576` are both accepted in `config.toml`.
///
/// Sizes add, subtract, sum, and scale by a plain count or by a [`Percent`]
/// (`ByteSize::gib(10) * Percent::new(10)` is `1GB`). Integer arithmetic saturates; see the
/// [module docs](super).
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
    SerializeDisplay,
    derive_more::From,
    derive_more::Into,
)]
pub struct ByteSize(u64);

impl ByteSize {
    pub const ZERO: Self = Self(0);
    pub const KIB: Self = Self(1 << 10);
    pub const MIB: Self = Self(1 << 20);
    pub const GIB: Self = Self(1 << 30);
    pub const TIB: Self = Self(1 << 40);

    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// `n` kibibytes: `n * 1024` bytes.
    #[must_use]
    pub const fn kib(n: u64) -> Self {
        Self(n << 10)
    }

    /// `n` mebibytes: `n * 1024^2` bytes.
    #[must_use]
    pub const fn mib(n: u64) -> Self {
        Self(n << 20)
    }

    /// `n` gibibytes: `n * 1024^3` bytes.
    #[must_use]
    pub const fn gib(n: u64) -> Self {
        Self(n << 30)
    }

    /// `n` tebibytes: `n * 1024^4` bytes.
    #[must_use]
    pub const fn tib(n: u64) -> Self {
        Self(n << 40)
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// A `du -h`-style rendering for showing sizes to people: `512B`, `1.5MB`, `16GB`.
    ///
    /// Unlike [`Display`](fmt::Display), this is lossy -- it keeps at most three significant
    /// digits and rounds *up* so a size never reads smaller than it is -- so it is unsuitable for
    /// the serialized form.
    #[must_use]
    pub fn human(self) -> HumanByteSize {
        HumanByteSize(self)
    }
}

impl Add for ByteSize {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for ByteSize {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl AddAssign for ByteSize {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for ByteSize {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

/// `n` times a size.
impl Mul<u64> for ByteSize {
    type Output = Self;

    fn mul(self, rhs: u64) -> Self {
        Self(self.0.saturating_mul(rhs))
    }
}

impl Mul<ByteSize> for u64 {
    type Output = ByteSize;

    fn mul(self, rhs: ByteSize) -> ByteSize {
        rhs * self
    }
}

/// A size split `n` ways, rounded down. Panics on zero, as integer division does.
impl Div<u64> for ByteSize {
    type Output = Self;

    fn div(self, rhs: u64) -> Self {
        Self(self.0 / rhs)
    }
}

/// A share of a size: `ByteSize::gib(10) * Percent::new(10)` is `1GB`.
impl Mul<Percent> for ByteSize {
    type Output = Self;

    fn mul(self, rhs: Percent) -> Self {
        Self(self.0 * rhs)
    }
}

impl Mul<ByteSize> for Percent {
    type Output = ByteSize;

    fn mul(self, rhs: ByteSize) -> ByteSize {
        rhs * self
    }
}

impl Sum for ByteSize {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Self> for ByteSize {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.copied().sum()
    }
}

/// More fractional digits than this cannot change the floored byte count of any `u64` size, and
/// keeping the count bounded keeps `10^digits` inside a `u128`.
const MAX_FRACTION_DIGITS: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    B,
    K,
    M,
    G,
    T,
}

impl Unit {
    /// Largest first, so the first unit that divides a value evenly is the one to display.
    const DESCENDING: [Self; 5] = [Self::T, Self::G, Self::M, Self::K, Self::B];

    const fn multiplier(self) -> u64 {
        match self {
            Self::B => 1,
            Self::K => 1 << 10,
            Self::M => 1 << 20,
            Self::G => 1 << 30,
            Self::T => 1 << 40,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::B => "B",
            Self::K => "KB",
            Self::M => "MB",
            Self::G => "GB",
            Self::T => "TB",
        }
    }

    /// `suffix` is whatever followed the number, already trimmed. An empty suffix is bytes.
    fn parse(suffix: &str) -> Option<Self> {
        Some(match suffix.to_ascii_lowercase().as_str() {
            "" | "b" => Self::B,
            "k" | "kb" | "kib" => Self::K,
            "m" | "mb" | "mib" => Self::M,
            "g" | "gb" | "gib" => Self::G,
            "t" | "tb" | "tib" => Self::T,
            _ => return None,
        })
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            return f.write_str("0B");
        }
        let unit = Unit::DESCENDING
            .into_iter()
            .find(|unit| self.0.is_multiple_of(unit.multiplier()))
            .expect("every value is divisible by one byte");
        write!(f, "{}{}", self.0 / unit.multiplier(), unit.label())
    }
}

/// The [`Display`](fmt::Display) rendering returned by [`ByteSize::human`].
#[derive(Clone, Copy, Debug)]
pub struct HumanByteSize(ByteSize);

impl fmt::Display for HumanByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0.bytes();
        // Anything below a kilobyte (zero included) has no fraction to show: print the raw count.
        let Some(unit) = Unit::DESCENDING
            .into_iter()
            .find(|unit| *unit != Unit::B && bytes >= unit.multiplier())
        else {
            return write!(f, "{bytes}B");
        };
        let multiplier = unit.multiplier();
        // At ten units and above, `du -h` drops the decimal; below it, it shows exactly one.
        // Either way it rounds up, so a size never reads smaller than it is.
        if bytes / multiplier >= 10 {
            return write!(f, "{}{}", bytes.div_ceil(multiplier), unit.label());
        }
        // `bytes < 10 * multiplier` here, so `bytes * 10` stays well inside a `u64`.
        let tenths = (bytes * 10).div_ceil(multiplier);
        if tenths >= 100 {
            write!(f, "{}{}", bytes.div_ceil(multiplier), unit.label())
        } else {
            write!(f, "{}.{}{}", tenths / 10, tenths % 10, unit.label())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ByteSizeParseError {
    #[error("expected a size like `1MB`, got an empty string")]
    Empty,
    #[error("`{0}` is not a number")]
    InvalidNumber(String),
    #[error("`{0}` is not a size unit; expected one of B, KB, MB, GB, TB")]
    UnknownUnit(String),
    #[error("a size in bytes must be a whole number")]
    FractionalBytes,
    #[error("size is too large to represent")]
    Overflow,
}

impl FromStr for ByteSize {
    type Err = ByteSizeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ByteSizeParseError::Empty);
        }

        let unit_start = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
        let (number, suffix) = s.split_at(unit_start);
        if number.is_empty() {
            return Err(ByteSizeParseError::InvalidNumber(s.to_owned()));
        }
        let suffix = suffix.trim();
        let unit = Unit::parse(suffix)
            .ok_or_else(|| ByteSizeParseError::UnknownUnit(suffix.to_owned()))?;

        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if (whole.is_empty() && fraction.is_empty()) || fraction.len() > MAX_FRACTION_DIGITS {
            return Err(ByteSizeParseError::InvalidNumber(number.to_owned()));
        }
        if unit == Unit::B && fraction.bytes().any(|digit| digit != b'0') {
            return Err(ByteSizeParseError::FractionalBytes);
        }

        // Each half is a run of ASCII digits; an absent half (`1.`, `.5`) is zero.
        let parse_digits = |digits: &str| -> Result<u128, ByteSizeParseError> {
            if digits.is_empty() {
                return Ok(0);
            }
            digits.parse().map_err(|e: ParseIntError| match e.kind() {
                IntErrorKind::PosOverflow => ByteSizeParseError::Overflow,
                _ => ByteSizeParseError::InvalidNumber(number.to_owned()),
            })
        };
        let whole = parse_digits(whole)?;
        let fraction_value = parse_digits(fraction)?;
        let scale =
            10u128.pow(u32::try_from(fraction.len()).expect("bounded by MAX_FRACTION_DIGITS"));
        let multiplier = u128::from(unit.multiplier());

        // `fraction_value < 10^18` and `multiplier <= 2^40`, so the product fits a `u128`; only
        // the whole part can overflow.
        let bytes = whole
            .checked_mul(multiplier)
            .and_then(|whole| whole.checked_add(fraction_value * multiplier / scale))
            .ok_or(ByteSizeParseError::Overflow)?;
        u64::try_from(bytes).map(Self).map_err(|_| ByteSizeParseError::Overflow)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        text_or_bytes::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::bare_bytes("4096", 4096)]
    #[case::explicit_bytes("4096B", 4096)]
    #[case::kilobytes("1KB", 1 << 10)]
    #[case::megabytes("1MB", 1 << 20)]
    #[case::gigabytes("10GB", 10 << 30)]
    #[case::terabytes("2TB", 2 << 40)]
    #[case::iec_spelling("1MiB", 1 << 20)]
    #[case::bare_unit_letter("1M", 1 << 20)]
    #[case::lowercase("1mb", 1 << 20)]
    #[case::mixed_case("1Mb", 1 << 20)]
    #[case::space_before_unit("1 MB", 1 << 20)]
    #[case::surrounding_whitespace("  1MB\n", 1 << 20)]
    #[case::fraction("1.5MB", 3 << 19)]
    #[case::fraction_without_leading_digit(".5KB", 512)]
    #[case::trailing_dot("1.KB", 1024)]
    // 1.3 * 1024 = 1331.2: fractions of a byte are dropped
    #[case::fraction_rounds_down("1.3KB", 1331)]
    #[case::whole_fraction_of_bytes("1.0B", 1)]
    #[case::zero("0", 0)]
    #[case::zero_with_unit("0GB", 0)]
    #[case::max("18446744073709551615", u64::MAX)]
    fn parses_sizes(#[case] input: &str, #[case] expected: u64) {
        assert_eq!(input.parse::<ByteSize>().unwrap(), ByteSize::from_bytes(expected));
    }

    #[rstest]
    #[case::empty("", ByteSizeParseError::Empty)]
    #[case::only_whitespace("  ", ByteSizeParseError::Empty)]
    #[case::unit_without_number("MB", ByteSizeParseError::InvalidNumber("MB".into()))]
    #[case::negative("-1MB", ByteSizeParseError::InvalidNumber("-1MB".into()))]
    #[case::plus_sign("+1MB", ByteSizeParseError::InvalidNumber("+1MB".into()))]
    #[case::two_dots("1.2.3MB", ByteSizeParseError::InvalidNumber("1.2.3".into()))]
    #[case::lone_dot(".", ByteSizeParseError::InvalidNumber(".".into()))]
    #[case::too_many_fraction_digits(
        "1.0000000000000000001KB",
        ByteSizeParseError::InvalidNumber("1.0000000000000000001".into())
    )]
    #[case::unknown_unit("1XB", ByteSizeParseError::UnknownUnit("XB".into()))]
    #[case::decimal_si_is_not_a_thing_here("1 parsec", ByteSizeParseError::UnknownUnit("parsec".into()))]
    #[case::fractional_bytes("1.5", ByteSizeParseError::FractionalBytes)]
    #[case::fractional_bytes_with_unit("1.5B", ByteSizeParseError::FractionalBytes)]
    // 2^64 bytes is 16777216TB
    #[case::overflow("16777216TB", ByteSizeParseError::Overflow)]
    #[case::overflow_in_bytes("18446744073709551616", ByteSizeParseError::Overflow)]
    #[case::absurd_number(
        "1000000000000000000000000000000000000000B",
        ByteSizeParseError::Overflow
    )]
    fn rejects_invalid_sizes(#[case] input: &str, #[case] expected: ByteSizeParseError) {
        assert_eq!(input.parse::<ByteSize>(), Err(expected));
    }

    #[rstest]
    #[case::zero(0, "0B")]
    #[case::bytes(1000, "1000B")]
    #[case::kilobytes(1024, "1KB")]
    #[case::megabytes(1 << 20, "1MB")]
    #[case::not_a_whole_megabyte(3 << 19, "1536KB")]
    #[case::gigabytes(10 << 30, "10GB")]
    #[case::terabytes(1 << 40, "1TB")]
    #[case::max(u64::MAX, "18446744073709551615B")]
    fn displays_the_largest_exact_unit(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(ByteSize::from_bytes(bytes).to_string(), expected);
    }

    #[rstest]
    #[case::zero(0, "0B")]
    #[case::bytes(512, "512B")]
    #[case::just_under_a_kilobyte(1023, "1023B")]
    // du -h prints exact multiples with one decimal, e.g. "1.0K".
    #[case::exact_kilobyte(1 << 10, "1.0KB")]
    #[case::half_a_kilobyte_more(3 << 9, "1.5KB")]
    // Rounds up: 1025 / 1024 = 1.0009..., ceiled to one decimal.
    #[case::rounds_up_to_one_decimal(1025, "1.1KB")]
    // 4095 and 4096 both land on 4.0KB once ceiled to one decimal.
    #[case::ceiling_collapses_neighbours_low(4095, "4.0KB")]
    #[case::ceiling_collapses_neighbours_high(4096, "4.0KB")]
    #[case::one_byte_over_four_kib(4097, "4.1KB")]
    #[case::megabytes(1_500_000, "1.5MB")]
    // At ten units and above, du -h drops the decimal.
    #[case::ten_gibibytes(10 << 30, "10GB")]
    #[case::rounds_up_to_a_whole_unit(16_642_998_272, "16GB")]
    #[case::exact_terabyte(1 << 40, "1.0TB")]
    // The largest defined unit is TB, so huge values stay in terabytes.
    #[case::max(u64::MAX, "16777216TB")]
    fn displays_human_readable_sizes(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(ByteSize::from_bytes(bytes).human().to_string(), expected);
    }

    #[rstest]
    #[case::text(r#""1MB""#, 1 << 20)]
    #[case::integer("1048576", 1 << 20)]
    #[case::zero("0", 0)]
    fn deserializes_from_text_or_an_integer(#[case] json: &str, #[case] expected: u64) {
        let size: ByteSize = serde_json::from_str(json).unwrap();
        assert_eq!(size, ByteSize::from_bytes(expected));
    }

    #[rstest]
    #[case::negative("-1")]
    #[case::float("1.5")]
    #[case::bool("true")]
    #[case::bad_text(r#""lots""#)]
    fn rejects_other_json_values(#[case] json: &str) {
        assert!(serde_json::from_str::<ByteSize>(json).is_err());
    }

    #[rstest]
    fn serializes_as_text() {
        assert_eq!(serde_json::to_string(&ByteSize::MIB).unwrap(), r#""1MB""#);
    }

    #[rstest]
    fn constants_are_powers_of_1024() {
        assert_eq!(ByteSize::ZERO.bytes(), 0);
        assert_eq!(ByteSize::KIB.bytes(), 1 << 10);
        assert_eq!(ByteSize::MIB.bytes(), 1 << 20);
        assert_eq!(ByteSize::GIB.bytes(), 1 << 30);
        assert_eq!(ByteSize::TIB.bytes(), 1 << 40);
        assert_eq!(ByteSize::default(), ByteSize::ZERO);
    }

    #[rstest]
    fn unit_constructors_scale_by_1024() {
        assert_eq!(ByteSize::kib(1), ByteSize::KIB);
        assert_eq!(ByteSize::mib(1), ByteSize::MIB);
        assert_eq!(ByteSize::gib(1), ByteSize::GIB);
        assert_eq!(ByteSize::tib(1), ByteSize::TIB);
        assert_eq!(ByteSize::kib(512).bytes(), 512 << 10);
        assert_eq!(ByteSize::mib(2).bytes(), 2 << 20);
        assert_eq!(ByteSize::gib(10).bytes(), 10 << 30);
    }

    proptest! {
        #[test]
        fn display_round_trips(bytes in any::<u64>()) {
            let size = ByteSize::from_bytes(bytes);
            prop_assert_eq!(size.to_string().parse::<ByteSize>().unwrap(), size);
        }

        #[test]
        fn serde_round_trips(bytes in any::<u64>()) {
            let size = ByteSize::from_bytes(bytes);
            let json = serde_json::to_string(&size).unwrap();
            prop_assert_eq!(serde_json::from_str::<ByteSize>(&json).unwrap(), size);
        }

        #[test]
        fn u64_conversions_are_lossless(bytes in any::<u64>()) {
            let size = ByteSize::from(bytes);
            prop_assert_eq!(u64::from(size), bytes);
            prop_assert_eq!(size.bytes(), bytes);
        }
    }

    #[rstest]
    fn sizes_add_subtract_and_sum() {
        assert_eq!(ByteSize::KIB + ByteSize::KIB, ByteSize::kib(2));
        assert_eq!(ByteSize::MIB - ByteSize::KIB, ByteSize::kib(1023));
        // saturating, like every other operation here
        assert_eq!(ByteSize::KIB - ByteSize::MIB, ByteSize::ZERO);
        assert_eq!(ByteSize::from_bytes(u64::MAX) + ByteSize::KIB, ByteSize::from_bytes(u64::MAX));

        let mut size = ByteSize::MIB;
        size += ByteSize::KIB;
        size -= ByteSize::kib(2);
        assert_eq!(size, ByteSize::kib(1023));

        let sizes = [ByteSize::KIB, ByteSize::MIB, ByteSize::GIB];
        assert_eq!(
            sizes.iter().sum::<ByteSize>(),
            ByteSize::from_bytes((1 << 10) + (1 << 20) + (1 << 30))
        );
        assert_eq!(sizes.into_iter().sum::<ByteSize>(), sizes.iter().sum());
        assert_eq!(std::iter::empty::<ByteSize>().sum::<ByteSize>(), ByteSize::ZERO);
    }

    #[rstest]
    fn sizes_scale_by_a_count() {
        assert_eq!(ByteSize::KIB * 4, ByteSize::kib(4));
        assert_eq!(4 * ByteSize::KIB, ByteSize::kib(4));
        assert_eq!(ByteSize::GIB * u64::MAX, ByteSize::from_bytes(u64::MAX));
        assert_eq!(ByteSize::kib(4) / 4, ByteSize::KIB);
        assert_eq!(ByteSize::from_bytes(7) / 2, ByteSize::from_bytes(3));
    }

    #[rstest]
    #[case::a_tenth(10, 10 << 30, 1 << 30)]
    #[case::the_whole(100, 1 << 20, 1 << 20)]
    #[case::more_than_the_whole(150, 1 << 20, 3 << 19)]
    #[case::rounds_down(1, 150, 1)]
    #[case::saturates(200, u64::MAX, u64::MAX)]
    fn sizes_scale_by_a_percent_in_either_order(
        #[case] pct: u64,
        #[case] bytes: u64,
        #[case] expected: u64,
    ) {
        let size = ByteSize::from_bytes(bytes);
        assert_eq!(size * Percent::new(pct), ByteSize::from_bytes(expected));
        assert_eq!(Percent::new(pct) * size, ByteSize::from_bytes(expected));
    }
}
