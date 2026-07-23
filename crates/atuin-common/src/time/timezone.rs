//! The local UTC offset, and the configurable timezone spec that resolves against it.

use std::str::FromStr;

use serde::Serialize;
use serde_with::DeserializeFromStr;
use time::{UtcOffset, format_description::FormatItem, macros::format_description};

/// The system's current UTC offset, falling back to UTC if it cannot be determined.
///
/// Safe to call from anywhere, including async code. `time` used to refuse this on a
/// multithreaded process, but since 0.3.37 only [`time::util::refresh_tz`] -- which
/// re-reads `$TZ` -- carries that restriction. We never re-read `$TZ`, so a changed
/// timezone is simply picked up on the next run.
pub fn local_offset_or_utc() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

/// Wrapper around [`UtcOffset`] supporting a wider variety of timezone formats.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, DeserializeFromStr, Serialize, derive_more::Display,
)]
#[display("{_0}")]
pub struct Timezone(pub UtcOffset);

/// format: `<+|-><hour>[:<minute>[:<second>]]`
static OFFSET_FMT: &[FormatItem<'_>] = format_description!(
    "[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional [:[offset_second padding:none]]]]]"
);

impl FromStr for Timezone {
    type Err = eyre::Report;

    fn from_str(s: &str) -> eyre::Result<Self> {
        // local timezone
        if matches!(s.to_lowercase().as_str(), "l" | "local") {
            // There have been some timezone issues, related to errors fetching it on some
            // platforms
            // Rather than fail to start, fallback to UTC. The user should still be able to specify
            // their timezone manually in the config file.
            return Ok(Self(local_offset_or_utc()));
        }

        if matches!(s.to_lowercase().as_str(), "0" | "utc") {
            let offset = UtcOffset::UTC;
            return Ok(Self(offset));
        }

        // offset from UTC
        if let Ok(offset) = UtcOffset::parse(s, OFFSET_FMT) {
            return Ok(Self(offset));
        }

        // IDEA: Currently named timezones are not supported, because the well-known crate
        // for this is `chrono_tz`, which is not really interoperable with the datetime crate
        // that we currently use - `time`. If ever we migrate to using `chrono`, this would
        // be a good feature to add.

        eyre::bail!(r#""{s}" is not a valid timezone spec"#);
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
