//! A count of bytes with a human-friendly text form.

use std::fmt;
use std::iter::Sum;
use std::num::{IntErrorKind, ParseIntError};
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};
use std::str::FromStr;

use easy_cast::{ConvTo, Trunc};
use serde::de::{Deserialize, Deserializer, Error, Visitor};
use serde_with::SerializeDisplay;

use super::Percent;

/// A number of bytes.
///
/// The text form is `<number><unit>`: `1MB`, `512 KB`, `1.5GiB`, or a bare `4096` for bytes.
/// Units follow SI: `KB`, `MB`, `GB`, `TB` are powers of 1000, and the IEC spellings `KiB`,
/// `MiB`, `GiB`, `TiB` are powers of 1024. A bare `K`, `M`, `G`, `T` is binary too, as it is for
/// `dd` and `du -h`. Case does not matter.
/// Fractions of a unit are floored to whole bytes; fractions of a *byte* are rejected.
///
/// Serializes as its text form. Deserializes from either the text form or a bare integer, so
/// `max_output_size = "1MB"` and `max_output_size = 1048576` are both accepted in `config.toml`.
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
    pub const KB: Self = Self(1_000);
    pub const MB: Self = Self(1_000_000);
    pub const GB: Self = Self(1_000_000_000);
    pub const TB: Self = Self(1_000_000_000_000);
    pub const KIB: Self = Self(1 << 10);
    pub const MIB: Self = Self(1 << 20);
    pub const GIB: Self = Self(1 << 30);
    pub const TIB: Self = Self(1 << 40);

    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// `n` kilobytes: `n * 1000` bytes.
    #[must_use]
    pub const fn kb(n: u64) -> Self {
        Self(n.saturating_mul(Self::KB.0))
    }

    /// `n` megabytes: `n * 1000^2` bytes.
    #[must_use]
    pub const fn mb(n: u64) -> Self {
        Self(n.saturating_mul(Self::MB.0))
    }

    /// `n` gigabytes: `n * 1000^3` bytes.
    #[must_use]
    pub const fn gb(n: u64) -> Self {
        Self(n.saturating_mul(Self::GB.0))
    }

    /// `n` terabytes: `n * 1000^4` bytes.
    #[must_use]
    pub const fn tb(n: u64) -> Self {
        Self(n.saturating_mul(Self::TB.0))
    }

    /// `n` kibibytes: `n * 1024` bytes.
    #[must_use]
    pub const fn kib(n: u64) -> Self {
        Self(n.saturating_mul(Self::KIB.0))
    }

    /// `n` mebibytes: `n * 1024^2` bytes.
    #[must_use]
    pub const fn mib(n: u64) -> Self {
        Self(n.saturating_mul(Self::MIB.0))
    }

    /// `n` gibibytes: `n * 1024^3` bytes.
    #[must_use]
    pub const fn gib(n: u64) -> Self {
        Self(n.saturating_mul(Self::GIB.0))
    }

    /// `n` tebibytes: `n * 1024^4` bytes.
    #[must_use]
    pub const fn tib(n: u64) -> Self {
        Self(n.saturating_mul(Self::TIB.0))
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// A `du --si`-style rendering for showing sizes to people: `512B`, `1.5MB`, `16GB`. Always in
    /// the decimal units, since that is how disks are sold and how Finder and
    /// Explorer report them.
    ///
    /// Unlike [`Display`](fmt::Display), this is lossy -- it keeps at most three significant
    /// digits and rounds *up* so a size never reads smaller than it is.
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

/// A share of a size: `ByteSize::gib(10) * Percent::new(10.0)` is `1GB`.
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

/// A size scaled by a factor, truncated to whole bytes and saturating at the bounds. A negative
/// or NaN factor gives zero.
impl Mul<f64> for ByteSize {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let scaled = self.0 as f64 * rhs;
        Self(u64::try_conv_to(Trunc, scaled).unwrap_or(if scaled > 0.0 {
            u64::MAX
        } else {
            0
        }))
    }
}

impl Mul<ByteSize> for f64 {
    type Output = ByteSize;

    fn mul(self, rhs: ByteSize) -> ByteSize {
        rhs * self
    }
}

impl Mul<f32> for ByteSize {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        self * f64::from(rhs)
    }
}

impl Mul<ByteSize> for f32 {
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
    Kilo,
    Mega,
    Giga,
    Tera,
    Kibi,
    Mebi,
    Gibi,
    Tebi,
}

impl Unit {
    /// Every unit, largest first, so the first that divides a value evenly is the one to
    /// display. Within a tier the decimal unit comes first: `512KB` is also exactly `500KiB`,
    /// and it should read back the way people write it.
    const DESCENDING: [Self; 9] = [
        Self::Tera,
        Self::Tebi,
        Self::Giga,
        Self::Gibi,
        Self::Mega,
        Self::Mebi,
        Self::Kilo,
        Self::Kibi,
        Self::B,
    ];

    /// The decimal units, largest first, for [`HumanByteSize`].
    const DECIMAL_DESCENDING: [Self; 4] = [Self::Tera, Self::Giga, Self::Mega, Self::Kilo];

    const fn multiplier(self) -> u64 {
        match self {
            Self::B => 1,
            Self::Kilo => ByteSize::KB.0,
            Self::Mega => ByteSize::MB.0,
            Self::Giga => ByteSize::GB.0,
            Self::Tera => ByteSize::TB.0,
            Self::Kibi => ByteSize::KIB.0,
            Self::Mebi => ByteSize::MIB.0,
            Self::Gibi => ByteSize::GIB.0,
            Self::Tebi => ByteSize::TIB.0,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::B => "B",
            Self::Kilo => "KB",
            Self::Mega => "MB",
            Self::Giga => "GB",
            Self::Tera => "TB",
            Self::Kibi => "KiB",
            Self::Mebi => "MiB",
            Self::Gibi => "GiB",
            Self::Tebi => "TiB",
        }
    }

    /// `suffix` is whatever followed the number, already trimmed. An empty suffix is bytes.
    fn parse(suffix: &str) -> Option<Self> {
        Some(match suffix.to_ascii_lowercase().as_str() {
            "" | "b" => Self::B,
            "kb" => Self::Kilo,
            "mb" => Self::Mega,
            "gb" => Self::Giga,
            "tb" => Self::Tera,
            "k" | "kib" => Self::Kibi,
            "m" | "mib" => Self::Mebi,
            "g" | "gib" => Self::Gibi,
            "t" | "tib" => Self::Tebi,
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

        let Some(unit) =
            Unit::DECIMAL_DESCENDING.into_iter().find(|unit| bytes >= unit.multiplier())
        else {
            return write!(f, "{bytes}B");
        };

        let multiplier = unit.multiplier();
        if bytes / multiplier >= 10 {
            return write!(f, "{}{}", bytes.div_ceil(multiplier), unit.label());
        }

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
    #[error("`{0}` is not a size unit; expected one of B, KB, MB, GB, TB, KiB, MiB, GiB, TiB")]
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
        struct ByteSizeVisitor;

        impl Visitor<'_> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a size like \"1MB\" or a number of bytes")
            }

            fn visit_u64<E: Error>(self, bytes: u64) -> Result<ByteSize, E> {
                Ok(ByteSize::from_bytes(bytes))
            }

            fn visit_i64<E: Error>(self, bytes: i64) -> Result<ByteSize, E> {
                let bytes =
                    u64::try_from(bytes).map_err(|_| E::custom("a size cannot be negative"))?;
                self.visit_u64(bytes)
            }

            fn visit_str<E: Error>(self, text: &str) -> Result<ByteSize, E> {
                text.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
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
    #[case::kilobytes("1KB", 1_000)]
    #[case::megabytes("1MB", 1_000_000)]
    #[case::gigabytes("10GB", 10_000_000_000)]
    #[case::terabytes("2TB", 2_000_000_000_000)]
    #[case::bare_unit_letter_is_binary("1M", 1 << 20)]
    #[case::lowercase_bare_unit_letter("1g", 1 << 30)]
    #[case::kibibytes("1KiB", 1 << 10)]
    #[case::mebibytes("1MiB", 1 << 20)]
    #[case::gibibytes("10GiB", 10 << 30)]
    #[case::tebibytes("2TiB", 2 << 40)]
    #[case::lowercase("1mb", 1_000_000)]
    #[case::lowercase_iec("1mib", 1 << 20)]
    #[case::mixed_case("1Mb", 1_000_000)]
    #[case::space_before_unit("1 MB", 1_000_000)]
    #[case::surrounding_whitespace("  1MB\n", 1_000_000)]
    #[case::fraction("1.5MB", 1_500_000)]
    #[case::fraction_of_a_binary_unit("1.5MiB", 3 << 19)]
    #[case::fraction_without_leading_digit(".5KB", 500)]
    #[case::trailing_dot("1.KB", 1000)]
    // 1.3 * 1024 = 1331.2: fractions of a byte are dropped
    #[case::fraction_rounds_down("1.3KiB", 1331)]
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
    #[case::unknown_word("1 parsec", ByteSizeParseError::UnknownUnit("parsec".into()))]
    #[case::fractional_bytes("1.5", ByteSizeParseError::FractionalBytes)]
    #[case::fractional_bytes_with_unit("1.5B", ByteSizeParseError::FractionalBytes)]
    // 2^64 bytes is 16777216TiB, or a little over 18446744TB
    #[case::overflow("16777216TiB", ByteSizeParseError::Overflow)]
    #[case::overflow_decimal("18446745TB", ByteSizeParseError::Overflow)]
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
    #[case::bytes(999, "999B")]
    #[case::kilobytes(1_000, "1KB")]
    #[case::kibibytes(1024, "1KiB")]
    #[case::megabytes(1_000_000, "1MB")]
    #[case::mebibytes(1 << 20, "1MiB")]
    #[case::not_a_whole_mebibyte(3 << 19, "1536KiB")]
    // 1536000 is both 1536KB and 1500KiB; the decimal unit is checked first
    #[case::exact_in_both_families(1_536_000, "1536KB")]
    #[case::gigabytes(10_000_000_000, "10GB")]
    #[case::gibibytes(10 << 30, "10GiB")]
    #[case::terabytes(2_000_000_000_000, "2TB")]
    #[case::tebibytes(1 << 40, "1TiB")]
    #[case::max(u64::MAX, "18446744073709551615B")]
    fn displays_the_largest_exact_unit(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(ByteSize::from_bytes(bytes).to_string(), expected);
    }

    #[rstest]
    #[case::zero(0, "0B")]
    #[case::bytes(512, "512B")]
    #[case::just_under_a_kilobyte(999, "999B")]
    // du prints exact multiples with one decimal, e.g. "1.0K".
    #[case::exact_kilobyte(1_000, "1.0KB")]
    #[case::half_a_kilobyte_more(1_500, "1.5KB")]
    // Rounds up: 1001 / 1000 = 1.001, ceiled to one decimal.
    #[case::rounds_up_to_one_decimal(1_001, "1.1KB")]
    // 3999 and 4000 both land on 4.0KB once ceiled to one decimal.
    #[case::ceiling_collapses_neighbours_low(3_999, "4.0KB")]
    #[case::ceiling_collapses_neighbours_high(4_000, "4.0KB")]
    #[case::one_byte_over_four_kb(4_001, "4.1KB")]
    #[case::megabytes(1_500_000, "1.5MB")]
    // A hair under ten units ceils past the last tenth, and reads as a whole ten.
    #[case::ceiling_reaches_the_next_whole_unit(9_999_999, "10MB")]
    // At ten units and above, du drops the decimal.
    #[case::ten_gigabytes(10_000_000_000, "10GB")]
    #[case::rounds_up_to_a_whole_unit(15_500_000_001, "16GB")]
    #[case::a_binary_unit_is_not_round_here(1 << 30, "1.1GB")]
    #[case::exact_terabyte(1_000_000_000_000, "1.0TB")]
    // The largest defined unit is TB, so huge values stay in terabytes.
    #[case::max(u64::MAX, "18446745TB")]
    fn displays_human_readable_sizes(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(ByteSize::from_bytes(bytes).human().to_string(), expected);
    }

    #[rstest]
    #[case::text(r#""1MB""#, 1_000_000)]
    #[case::iec_text(r#""1MiB""#, 1 << 20)]
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
        assert_eq!(serde_json::to_string(&ByteSize::MB).unwrap(), r#""1MB""#);
        assert_eq!(serde_json::to_string(&ByteSize::MIB).unwrap(), r#""1MiB""#);
    }

    #[rstest]
    fn constants_follow_si() {
        assert_eq!(ByteSize::ZERO.bytes(), 0);
        assert_eq!(ByteSize::KB.bytes(), 1_000);
        assert_eq!(ByteSize::MB.bytes(), 1_000_000);
        assert_eq!(ByteSize::GB.bytes(), 1_000_000_000);
        assert_eq!(ByteSize::TB.bytes(), 1_000_000_000_000);
        assert_eq!(ByteSize::KIB.bytes(), 1 << 10);
        assert_eq!(ByteSize::MIB.bytes(), 1 << 20);
        assert_eq!(ByteSize::GIB.bytes(), 1 << 30);
        assert_eq!(ByteSize::TIB.bytes(), 1 << 40);
        assert_eq!(ByteSize::default(), ByteSize::ZERO);
    }

    #[rstest]
    fn unit_constructors_scale_by_their_unit() {
        assert_eq!(ByteSize::kb(1), ByteSize::KB);
        assert_eq!(ByteSize::mb(1), ByteSize::MB);
        assert_eq!(ByteSize::gb(1), ByteSize::GB);
        assert_eq!(ByteSize::tb(1), ByteSize::TB);
        assert_eq!(ByteSize::mb(500).bytes(), 500_000_000);
        assert_eq!(ByteSize::tb(u64::MAX), ByteSize::from_bytes(u64::MAX));
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
    #[case::a_tenth(10.0, 10 << 30, 1 << 30)]
    #[case::the_whole(100.0, 1 << 20, 1 << 20)]
    #[case::more_than_the_whole(150.0, 1 << 20, 3 << 19)]
    #[case::a_fraction_of_a_percent(2.5, 1000, 25)]
    #[case::rounds_down(1.0, 150, 1)]
    #[case::saturates(200.0, u64::MAX, u64::MAX)]
    fn sizes_scale_by_a_percent_in_either_order(
        #[case] pct: f64,
        #[case] bytes: u64,
        #[case] expected: u64,
    ) {
        let size = ByteSize::from_bytes(bytes);
        assert_eq!(size * Percent::new(pct), ByteSize::from_bytes(expected));
        assert_eq!(Percent::new(pct) * size, ByteSize::from_bytes(expected));
    }

    #[rstest]
    #[case::doubles(2.0, 1 << 20, 1 << 21)]
    #[case::halves(0.5, 1 << 20, 1 << 19)]
    #[case::rounds_down(0.1, 15, 1)]
    #[case::saturates(1e30, u64::MAX, u64::MAX)]
    #[case::negative_is_zero(-1.0, 1 << 20, 0)]
    #[case::nan_is_zero(f64::NAN, 1 << 20, 0)]
    fn sizes_scale_by_a_factor_in_either_order(
        #[case] factor: f64,
        #[case] bytes: u64,
        #[case] expected: u64,
    ) {
        let size = ByteSize::from_bytes(bytes);
        assert_eq!(size * factor, ByteSize::from_bytes(expected));
        assert_eq!(factor * size, ByteSize::from_bytes(expected));
        #[allow(clippy::cast_possible_truncation)]
        let factor = factor as f32;
        assert_eq!(size * factor, factor * size);
    }
}
