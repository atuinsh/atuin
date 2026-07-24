//! The local UTC offset, and the configurable timezone spec that resolves against it.

use std::str::FromStr;

use serde::Serialize;
use serde_with::DeserializeFromStr;
use time::{UtcOffset, format_description::FormatItem, macros::format_description};
use tracing::warn;

/// Extensions to [`UtcOffset`].
pub trait UtcOffsetExt {
    /// The system's current local UTC offset, falling back to UTC if it cannot be
    /// determined.
    ///
    /// Warns on the fallback, so rendering everything in UTC is at least traceable
    /// rather than silent. Prefer [`UtcOffset::current_local_offset`] where the caller
    /// can propagate the failure instead.
    fn local_or_utc() -> UtcOffset;
}

impl UtcOffsetExt for UtcOffset {
    fn local_or_utc() -> UtcOffset {
        UtcOffset::current_local_offset().unwrap_or_else(|e| {
            warn!("could not determine local UTC offset, falling back to UTC: {e}");
            UtcOffset::UTC
        })
    }
}

/// Wrapper around [`UtcOffset`] supporting a wider variety of timezone formats.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    DeserializeFromStr,
    Serialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into,
)]
#[display("{_0}")]
pub struct Timezone(pub UtcOffset);

/// format: `<+|-><hour>[:<minute>[:<second>]]`
static OFFSET_FMT: &[FormatItem<'_>] = format_description!(
    "[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional [:[offset_second padding:none]]]]]"
);

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum TimezoneDecodingError {
    #[error("failed to query local timezone {_0} ")]
    IndeterminateOffset(#[from] time::error::IndeterminateOffset),
    #[error("invalid timezone format: {_0}")]
    InvalidTimezone(#[from] time::error::Parse),
}

impl FromStr for Timezone {
    type Err = TimezoneDecodingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if matches!(s.to_lowercase().as_str(), "l" | "local") {
            return Ok(UtcOffset::current_local_offset()?.into());
        }

        if matches!(s.to_lowercase().as_str(), "0" | "utc") {
            return Ok(UtcOffset::UTC.into());
        }

        // IDEA: Currently named timezones are not supported, because the well-known crate for this
        // is `chrono_tz`, which is not really interoperable with the datetime crate that we
        // currently use - `time`. If ever we migrate to using `chrono`, this would be a good
        // feature to add.
        Ok(UtcOffset::parse(s, OFFSET_FMT)?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::local("local")]
    #[case::l("l")]
    #[case::utc("utc")]
    #[case::zero("0")]
    #[case::plus_offset("+09:30")]
    #[case::minus_offset("-2:30")]
    fn timezone_parses(#[case] spec: &str) {
        assert!(Timezone::from_str(spec).is_ok());
    }

    #[rstest]
    #[case::no_sign("09:30")]
    #[case::garbage("not-a-timezone")]
    fn timezone_rejects_invalid(#[case] spec: &str) {
        assert!(Timezone::from_str(spec).is_err());
    }
}
