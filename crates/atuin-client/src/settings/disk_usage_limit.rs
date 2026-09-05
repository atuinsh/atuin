//! How much disk a store may use: an absolute size, a share of the disk, or no limit.

use std::str::FromStr;

use atuin_common::units::{
    ByteSize, ByteSizeParseError, Percent, PercentParseError, text_or_bytes,
};
use serde::{Deserialize, Deserializer};
use serde_with::SerializeDisplay;

/// The most disk space something may use.
///
/// Written in `config.toml` as `"unlimited"`, a percentage of the disk such as `"10%"`, or a
/// [`ByteSize`] such as `"10GB"`. A bare integer is a byte count. The percentage is only
/// meaningful against a disk; see [`Self::resolve`].
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    SerializeDisplay,
    derive_more::Display,
    derive_more::From,
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

    /// The number of bytes this limit allows on a disk of `disk_size`, or `None` when there is
    /// no limit.
    ///
    /// An absolute [`Self::Bytes`] is returned as-is, even if it is larger than `disk_size`; the
    /// caller decides what a limit larger than the disk means.
    #[must_use]
    pub fn resolve(self, disk_size: ByteSize) -> Option<ByteSize> {
        match self {
            Self::Unlimited => None,
            Self::Bytes(bytes) => Some(bytes),
            Self::Percent(percent) => Some(disk_size * percent),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiskUsageLimitParseError {
    #[error("invalid percentage: {0}")]
    Percent(#[from] PercentParseError),
    #[error("expected `unlimited`, a percentage like `10%`, or a size like `10GB`: {0}")]
    Bytes(#[from] ByteSizeParseError),
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
        Ok(Self::Bytes(s.parse()?))
    }
}

impl<'de> Deserialize<'de> for DiskUsageLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        text_or_bytes::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn percent(value: u64) -> Percent {
        Percent::new(value)
    }

    fn bytes(value: u64) -> DiskUsageLimit {
        DiskUsageLimit::Bytes(ByteSize::from_bytes(value))
    }

    fn any_limit() -> impl Strategy<Value = DiskUsageLimit> {
        prop_oneof![
            Just(DiskUsageLimit::Unlimited),
            any::<u64>().prop_map(bytes),
            any::<u64>().prop_map(|value| DiskUsageLimit::Percent(percent(value))),
        ]
    }

    #[rstest]
    #[case::unlimited("unlimited", DiskUsageLimit::Unlimited)]
    #[case::unlimited_is_case_insensitive("Unlimited", DiskUsageLimit::Unlimited)]
    #[case::unlimited_with_whitespace(" unlimited\n", DiskUsageLimit::Unlimited)]
    #[case::percent("10%", DiskUsageLimit::Percent(percent(10)))]
    #[case::percent_with_whitespace(" 10 % ", DiskUsageLimit::Percent(percent(10)))]
    #[case::percent_above_the_disk("150%", DiskUsageLimit::Percent(percent(150)))]
    #[case::size("10GB", bytes(10 << 30))]
    #[case::size_with_fraction("1.5GB", bytes(3 << 29))]
    #[case::bare_bytes("4096", bytes(4096))]
    fn parses_every_form(#[case] input: &str, #[case] expected: DiskUsageLimit) {
        assert_eq!(input.parse::<DiskUsageLimit>().unwrap(), expected);
    }

    #[rstest]
    #[case::empty("", DiskUsageLimitParseError::Bytes(ByteSizeParseError::Empty))]
    #[case::fractional_percent(
        "2.5%",
        DiskUsageLimitParseError::Percent(PercentParseError::InvalidNumber("2.5".into()))
    )]
    #[case::unknown_unit(
        "10 parsecs",
        DiskUsageLimitParseError::Bytes(ByteSizeParseError::UnknownUnit("parsecs".into()))
    )]
    #[case::misspelt_unlimited(
        "unlimitd",
        DiskUsageLimitParseError::Bytes(ByteSizeParseError::InvalidNumber("unlimitd".into()))
    )]
    fn rejects_invalid_limits(#[case] input: &str, #[case] expected: DiskUsageLimitParseError) {
        assert_eq!(input.parse::<DiskUsageLimit>(), Err(expected));
    }

    #[rstest]
    #[case::unlimited(DiskUsageLimit::Unlimited, "unlimited")]
    #[case::size(bytes(10 << 30), "10GB")]
    #[case::percent(DiskUsageLimit::Percent(percent(10)), "10%")]
    fn displays_the_text_form(#[case] limit: DiskUsageLimit, #[case] expected: &str) {
        assert_eq!(limit.to_string(), expected);
    }

    #[rstest]
    #[case::unlimited_has_no_bound(DiskUsageLimit::Unlimited, 1 << 40, None)]
    #[case::bytes_are_absolute(bytes(10 << 30), 1 << 40, Some(10 << 30))]
    #[case::bytes_are_not_clamped_to_the_disk(bytes(10 << 30), 1 << 30, Some(10 << 30))]
    #[case::percent_is_a_share_of_the_disk(DiskUsageLimit::Percent(percent(10)), 1000, Some(100))]
    fn resolves_against_a_disk_size(
        #[case] limit: DiskUsageLimit,
        #[case] disk_size: u64,
        #[case] expected: Option<u64>,
    ) {
        assert_eq!(
            limit.resolve(ByteSize::from_bytes(disk_size)),
            expected.map(ByteSize::from_bytes)
        );
        assert_eq!(limit.is_unlimited(), expected.is_none());
    }

    #[rstest]
    #[case::text_unlimited(r#""unlimited""#, DiskUsageLimit::Unlimited)]
    #[case::text_percent(r#""10%""#, DiskUsageLimit::Percent(percent(10)))]
    #[case::text_size(r#""10GB""#, bytes(10 << 30))]
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
        assert_eq!(DiskUsageLimit::from(ByteSize::MIB), bytes(1 << 20));
        assert_eq!(DiskUsageLimit::from(percent(10)), DiskUsageLimit::Percent(percent(10)));
    }

    proptest! {
        #[test]
        fn display_round_trips(limit in any_limit()) {
            prop_assert_eq!(limit.to_string().parse::<DiskUsageLimit>().unwrap(), limit);
        }

        #[test]
        fn serde_round_trips(limit in any_limit()) {
            let json = serde_json::to_string(&limit).unwrap();
            prop_assert_eq!(serde_json::from_str::<DiskUsageLimit>(&json).unwrap(), limit);
        }
    }
}
