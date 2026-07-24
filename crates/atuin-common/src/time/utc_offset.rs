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
    fn local_or_utc() -> UtcOffset;

    /// Resolve a user-supplied timezone spec.
    ///
    /// Accepts `local`/`l`, queried from the system; `utc`/`0`; or an offset from UTC
    /// such as `+09:30` or `-2:30`.
    ///
    /// Named zones are deliberately not accepted -- see the note in the implementation.
    fn resolve_spec(spec: impl AsRef<str>) -> Result<UtcOffset, TimezoneDecodingError>;
}

impl UtcOffsetExt for UtcOffset {
    fn local_or_utc() -> UtcOffset {
        UtcOffset::current_local_offset().unwrap_or_else(|e| {
            warn!("could not determine local UTC offset, falling back to UTC: {e}");
            UtcOffset::UTC
        })
    }

    fn resolve_spec(spec: impl AsRef<str>) -> Result<UtcOffset, TimezoneDecodingError> {
        let spec = spec.as_ref().to_lowercase();

        if matches!(spec.as_str(), "l" | "local") {
            return Ok(UtcOffset::current_local_offset()?);
        }

        if matches!(spec.as_str(), "0" | "utc") {
            return Ok(UtcOffset::UTC);
        }

        // IDEA: Currently named timezones are not supported, because the well-known crate for this
        // is `chrono_tz`, which is not really interoperable with the datetime crate that we
        // currently use - `time`. If ever we migrate to using `chrono`, this would be a good
        // feature to add.
        Ok(UtcOffset::parse(&spec, OFFSET_FMT)?)
    }
}

/// A user-supplied timezone spec, resolved to a [`UtcOffset`].
///
/// [`UtcOffset`] is foreign, so it cannot implement `FromStr`/`Deserialize` here. This
/// newtype carries those impls for the config file and CLI flags; convert with
/// [`From`]/[`Into`] or read the wrapped offset directly.
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
pub struct UtcOffsetSpec(pub UtcOffset);

impl FromStr for UtcOffsetSpec {
    type Err = TimezoneDecodingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UtcOffset::resolve_spec(s)?.into())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::no_sign("09:30")]
    #[case::garbage("not-a-timezone")]
    fn resolve_spec_rejects_invalid(#[case] spec: &str) {
        assert!(UtcOffset::resolve_spec(spec).is_err());
        assert!(UtcOffsetSpec::from_str(spec).is_err());
    }

    #[test]
    fn spec_wraps_the_resolved_offset() {
        let spec = UtcOffsetSpec::from_str("+09:30").unwrap();
        assert_eq!(spec.0.as_hms(), (9, 30, 0));
        // derive_more gives the conversion both ways
        let offset: UtcOffset = spec.into();
        assert_eq!(UtcOffsetSpec::from(offset), spec);
    }

    #[rstest]
    #[case::utc("utc", 0, 0, 0)]
    #[case::zero("0", 0, 0, 0)]
    #[case::plus("+09:30", 9, 30, 0)]
    #[case::minus("-2:30", -2, -30, 0)]
    #[case::with_seconds("+01:23:45", 1, 23, 45)]
    // specs are case-insensitive
    #[case::uppercase("UTC", 0, 0, 0)]
    fn resolve_spec_returns_the_offset(
        #[case] spec: &str,
        #[case] h: i8,
        #[case] m: i8,
        #[case] s: i8,
    ) {
        assert_eq!(UtcOffset::resolve_spec(spec).unwrap().as_hms(), (h, m, s));
    }

    #[test]
    fn resolve_spec_accepts_anything_stringlike() {
        // the point of `impl AsRef<str>`: borrowed or owned, no dance at the call site
        assert!(UtcOffset::resolve_spec("utc").is_ok());
        assert!(UtcOffset::resolve_spec(String::from("utc")).is_ok());
    }

    #[test]
    fn resolve_spec_local_queries_the_system() {
        // cannot assert the value -- it depends on the machine -- but it must resolve
        assert!(UtcOffset::resolve_spec("local").is_ok());
    }
}
