//! How much disk a store may use: an absolute size, a share of the disk, or no limit.

use std::fmt;
use std::str::FromStr;

use atuin_common::units::{ByteSize, Percent, PercentParseError};
use serde::de::{Deserialize, Deserializer, Error, Visitor};
use serde_with::SerializeDisplay;

/// The most disk space something may use.
///
/// ```toml
/// field = "10KB"      # 10 * 1000 bytes
/// field = "1000"      # 1000 bytes
/// field = "10%"       # 10% of the total filesystem space
/// field = "unlimited" # Unlimited disk usage
/// ```
#[derive(
    Clone, Copy, Debug, PartialEq, SerializeDisplay, derive_more::Display, derive_more::From,
)]
pub enum DiskUsageLimit {
    /// Never limit by size.
    #[display("unlimited")]
    #[from(skip)]
    Unlimited,
    /// An absolute size, e.g. `10GB`.
    #[display("{_0}")]
    Bytes(ByteSize),
    /// A share of the disk the data lives on, e.g. `10%`.
    #[display("{_0}")]
    Percent(Percent),
}

impl DiskUsageLimit {
    #[must_use]
    pub const fn is_unlimited(self) -> bool {
        matches!(self, Self::Unlimited)
    }

    /// Number of bytes this allows on a disk of `disk_size`, or `None` when there is no limit.
    ///
    /// An absolute [`Self::Bytes`] is returned as-is, even if it is larger than `disk_size`.
    #[must_use]
    pub fn resolve(self, disk_size: ByteSize) -> Option<ByteSize> {
        match self {
            Self::Unlimited => None,
            Self::Bytes(bytes) => Some(bytes),
            Self::Percent(percent) => Some(ByteSize::b(disk_size.as_u64() * percent)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiskUsageLimitParseError {
    #[error("invalid percentage: {0}")]
    Percent(#[from] PercentParseError),
    #[error("expected `unlimited`, a percentage like `10%`, or a size like `10GB`: {0}")]
    Bytes(String),
}

impl FromStr for DiskUsageLimit {
    type Err = DiskUsageLimitParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("unlimited") {
            return Ok(Self::Unlimited);
        }
        if s.ends_with('%') {
            return Ok(Self::Percent(s.parse()?));
        }
        s.parse::<ByteSize>()
            .map(Self::Bytes)
            .map_err(|e| DiskUsageLimitParseError::Bytes(e.to_string()))
    }
}

impl<'de> Deserialize<'de> for DiskUsageLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DiskUsageLimitVisitor;

        impl Visitor<'_> for DiskUsageLimitVisitor {
            type Value = DiskUsageLimit;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "\"unlimited\", a percentage like \"10%\", a size like \"10GB\", or a number \
                     of bytes",
                )
            }

            fn visit_u64<E: Error>(self, bytes: u64) -> Result<DiskUsageLimit, E> {
                Ok(DiskUsageLimit::Bytes(ByteSize::b(bytes)))
            }

            fn visit_i64<E: Error>(self, bytes: i64) -> Result<DiskUsageLimit, E> {
                let bytes =
                    u64::try_from(bytes).map_err(|_| E::custom("a size cannot be negative"))?;
                self.visit_u64(bytes)
            }

            fn visit_str<E: Error>(self, text: &str) -> Result<DiskUsageLimit, E> {
                text.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(DiskUsageLimitVisitor)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn percent(value: f64) -> Percent {
        Percent::new(value)
    }

    fn bytes(value: u64) -> DiskUsageLimit {
        DiskUsageLimit::Bytes(ByteSize::b(value))
    }

    /// `Bytes` is excluded: `bytesize`'s one-decimal display is lossy, so a size does not survive a
    /// display round trip. `Unlimited` and `Percent` still do.
    fn any_lossless_limit() -> impl Strategy<Value = DiskUsageLimit> {
        prop_oneof![
            Just(DiskUsageLimit::Unlimited),
            (0.0..=f64::MAX).prop_map(|value| DiskUsageLimit::Percent(percent(value))),
        ]
    }

    #[rstest]
    #[case::unlimited("unlimited", DiskUsageLimit::Unlimited)]
    #[case::unlimited_is_case_insensitive("Unlimited", DiskUsageLimit::Unlimited)]
    #[case::unlimited_with_whitespace(" unlimited\n", DiskUsageLimit::Unlimited)]
    #[case::percent("10%", DiskUsageLimit::Percent(percent(10.0)))]
    #[case::percent_with_whitespace(" 10 % ", DiskUsageLimit::Percent(percent(10.0)))]
    #[case::percent_above_the_disk("150%", DiskUsageLimit::Percent(percent(150.0)))]
    #[case::fractional_percent("2.5%", DiskUsageLimit::Percent(percent(2.5)))]
    #[case::size("10GB", bytes(10_000_000_000))]
    #[case::binary_size("10GiB", bytes(10 << 30))]
    #[case::size_with_fraction("1.5GB", bytes(1_500_000_000))]
    #[case::bare_bytes("4096", bytes(4096))]
    fn parses_every_form(#[case] input: &str, #[case] expected: DiskUsageLimit) {
        assert_eq!(input.parse::<DiskUsageLimit>().unwrap(), expected);
    }

    #[test]
    fn rejects_a_negative_percent() {
        assert_eq!(
            "-5%".parse::<DiskUsageLimit>(),
            Err(DiskUsageLimitParseError::Percent(PercentParseError::Negative))
        );
    }

    #[rstest]
    #[case::empty("")]
    #[case::unknown_unit("10 parsecs")]
    #[case::misspelt_unlimited("unlimitd")]
    fn rejects_an_invalid_size(#[case] input: &str) {
        assert!(matches!(input.parse::<DiskUsageLimit>(), Err(DiskUsageLimitParseError::Bytes(_))));
    }

    #[rstest]
    #[case::unlimited(DiskUsageLimit::Unlimited, "unlimited")]
    #[case::size(bytes(10 << 30), "10.0 GiB")]
    #[case::percent(DiskUsageLimit::Percent(percent(10.0)), "10%")]
    fn displays_the_text_form(#[case] limit: DiskUsageLimit, #[case] expected: &str) {
        assert_eq!(limit.to_string(), expected);
    }

    #[rstest]
    #[case::unlimited_has_no_bound(DiskUsageLimit::Unlimited, 1 << 40, None)]
    #[case::bytes_are_absolute(bytes(10 << 30), 1 << 40, Some(10 << 30))]
    #[case::bytes_are_not_clamped_to_the_disk(bytes(10 << 30), 1 << 30, Some(10 << 30))]
    #[case::percent_is_a_share_of_the_disk(DiskUsageLimit::Percent(percent(10.0)), 1000, Some(100))]
    fn resolves_against_a_disk_size(
        #[case] limit: DiskUsageLimit,
        #[case] disk_size: u64,
        #[case] expected: Option<u64>,
    ) {
        assert_eq!(limit.resolve(ByteSize::b(disk_size)), expected.map(ByteSize::b));
        assert_eq!(limit.is_unlimited(), expected.is_none());
    }

    #[rstest]
    #[case::text_unlimited(r#""unlimited""#, DiskUsageLimit::Unlimited)]
    #[case::text_percent(r#""10%""#, DiskUsageLimit::Percent(percent(10.0)))]
    #[case::text_size(r#""10GB""#, bytes(10_000_000_000))]
    #[case::integer_bytes("1048576", bytes(1 << 20))]
    fn deserializes_from_text_or_an_integer(#[case] json: &str, #[case] expected: DiskUsageLimit) {
        assert_eq!(serde_json::from_str::<DiskUsageLimit>(json).unwrap(), expected);
    }

    #[rstest]
    #[case::negative("-1")]
    #[case::bool("false")]
    #[case::bad_text(r#""some""#)]
    fn rejects_other_json_values(#[case] json: &str) {
        assert!(serde_json::from_str::<DiskUsageLimit>(json).is_err());
    }

    #[rstest]
    fn converts_from_its_parts() {
        assert_eq!(DiskUsageLimit::from(ByteSize::mib(1)), bytes(1 << 20));
        assert_eq!(DiskUsageLimit::from(percent(10.0)), DiskUsageLimit::Percent(percent(10.0)));
    }

    proptest! {
        #[test]
        fn display_round_trips(limit in any_lossless_limit()) {
            prop_assert_eq!(limit.to_string().parse::<DiskUsageLimit>().unwrap(), limit);
        }

        #[test]
        fn serde_round_trips(limit in any_lossless_limit()) {
            let json = serde_json::to_string(&limit).unwrap();
            prop_assert_eq!(serde_json::from_str::<DiskUsageLimit>(&json).unwrap(), limit);
        }
    }
}
