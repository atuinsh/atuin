//! The local UTC offset, and the configurable timezone spec that resolves against it.

use std::str::FromStr;

use serde::{Serialize, Serializer};
use serde_with::DeserializeFromStr;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};
use tracing::warn;

/// Extensions to [`UtcOffset`].
pub trait UtcOffsetExt {
    /// The system's current local UTC offset, falling back to UTC if it cannot be
    /// determined.
    fn local_or_utc() -> UtcOffset;
}

impl UtcOffsetExt for UtcOffset {
    fn local_or_utc() -> UtcOffset {
        Self::current_local_offset().unwrap_or_else(|e| {
            warn!("could not determine local UTC offset, falling back to UTC: {e}");
            Self::UTC
        })
    }
}

/// A user-supplied timezone spec.
///
/// Unlike a plain [`UtcOffset`], `Local` is not resolved until [`UtcOffsetSpec::offset_at`] is
/// called for a specific instant. Resolving it once up front -- as this type used to, by
/// immediately collapsing into a bare [`UtcOffset`] -- freezes whichever DST period happened to
/// be active at that moment, which is simply wrong for any instant in the other period. `Fixed`
/// carries its offset unconditionally: asking for a fixed offset means "ignore DST".
#[derive(Clone, Copy, Debug, Eq, PartialEq, DeserializeFromStr, derive_more::Display)]
pub enum UtcOffsetSpec {
    /// Follow the system's local offset, resolved fresh for whatever instant it is asked about.
    #[display("local")]
    Local,
    /// A fixed offset from UTC, applied uniformly regardless of DST.
    #[display("{_0}")]
    Fixed(UtcOffset),
}

impl UtcOffsetSpec {
    /// The offset that applies at a given instant.
    ///
    /// `Local` re-queries the system for `at`'s own offset (DST-correct for `at`, not just for
    /// "now"), falling back to UTC if it cannot be determined. `Fixed` returns its pinned offset
    /// unconditionally, ignoring `at`.
    #[must_use]
    pub fn offset_at(self, at: OffsetDateTime) -> UtcOffset {
        match self {
            Self::Local => UtcOffset::local_offset_at(at).unwrap_or_else(|e| {
                warn!("could not determine local UTC offset, falling back to UTC: {e}");
                UtcOffset::UTC
            }),
            Self::Fixed(offset) => offset,
        }
    }
}

impl FromStr for UtcOffsetSpec {
    type Err = TimezoneDecodingError;

    /// Accepts `local`/`l`, resolved per-instant against the system; `utc`/`0`; or a fixed
    /// offset from UTC such as `+09:30` or `-2:30`.
    ///
    /// Named zones are deliberately not accepted -- see the note below.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let spec = s.to_lowercase();

        if matches!(spec.as_str(), "l" | "local") {
            return Ok(Self::Local);
        }

        if matches!(spec.as_str(), "0" | "utc") {
            return Ok(Self::Fixed(UtcOffset::UTC));
        }

        // IDEA: Currently named timezones are not supported, because the well-known crate for this
        // is `chrono_tz`, which is not really interoperable with the datetime crate that we
        // currently use - `time`. If ever we migrate to using `chrono`, this would be a good
        // feature to add.
        Ok(Self::Fixed(UtcOffset::parse(&spec, OFFSET_FMT)?))
    }
}

impl Default for UtcOffsetSpec {
    fn default() -> Self {
        Self::Fixed(UtcOffset::UTC)
    }
}

// Serialized as the same plain string the config file and CLI flags accept -- `"local"` or an
// offset like `"+09:30"` -- via the `Display` impl above, rather than the tagged representation
// `#[derive(Serialize)]` would give an enum. Paired with `DeserializeFromStr` above, this keeps
// the round trip through TOML/JSON symmetric with `FromStr`.
impl Serialize for UtcOffsetSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// format: `<+|-><hour>[:<minute>[:<second>]]`
static OFFSET_FMT: &[FormatItem<'_>] = format_description!(
    "[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional \
     [:[offset_second padding:none]]]]]"
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
    use rstest::rstest;
    use time::macros::datetime;

    use super::*;

    #[rstest]
    #[case::no_sign("09:30")]
    #[case::garbage("not-a-timezone")]
    fn from_str_rejects_invalid(#[case] spec: &str) {
        assert!(UtcOffsetSpec::from_str(spec).is_err());
    }

    #[rstest]
    #[case::lowercase("local")]
    #[case::short("l")]
    #[case::uppercase("LOCAL")]
    fn from_str_local_stays_symbolic(#[case] spec: &str) {
        // `Local` must not be eagerly resolved to a concrete offset here -- that eager
        // resolution, at settings-load time, was the root cause of the DST bug this type
        // exists to prevent (see `offset_at`'s tests below).
        assert_eq!(UtcOffsetSpec::from_str(spec).unwrap(), UtcOffsetSpec::Local);
    }

    #[rstest]
    #[case::utc("utc", 0, 0, 0)]
    #[case::zero("0", 0, 0, 0)]
    #[case::plus("+09:30", 9, 30, 0)]
    #[case::minus("-2:30", -2, -30, 0)]
    #[case::with_seconds("+01:23:45", 1, 23, 45)]
    // specs are case-insensitive
    #[case::uppercase("UTC", 0, 0, 0)]
    fn from_str_returns_a_fixed_offset(
        #[case] spec: &str,
        #[case] h: i8,
        #[case] m: i8,
        #[case] s: i8,
    ) {
        let UtcOffsetSpec::Fixed(offset) = UtcOffsetSpec::from_str(spec).unwrap() else {
            panic!("{spec:?} should resolve to a fixed offset");
        };
        assert_eq!(offset.as_hms(), (h, m, s));
    }

    #[rstest]
    fn display_round_trips_through_from_str() {
        for spec in ["local", "utc", "+09:30", "-02:30:00"] {
            let parsed = UtcOffsetSpec::from_str(spec).unwrap();
            assert_eq!(UtcOffsetSpec::from_str(&parsed.to_string()).unwrap(), parsed);
        }
    }

    #[rstest]
    fn fixed_offset_at_ignores_the_instant() {
        let spec = UtcOffsetSpec::Fixed(UtcOffset::from_hms(-5, 0, 0).unwrap());
        assert_eq!(spec.offset_at(datetime!(2026-01-15 00:00 UTC)).as_hms(), (-5, 0, 0));
        assert_eq!(spec.offset_at(datetime!(2026-08-15 00:00 UTC)).as_hms(), (-5, 0, 0));
    }

    #[rstest]
    fn local_offset_at_queries_the_system_for_the_given_instant() {
        // Cannot assert a specific value -- it depends on the machine running the test -- but
        // it must resolve, and must not panic, for an arbitrary instant in either direction from
        // "now", which is the whole point of taking `at` instead of always querying "now".
        let _ = UtcOffsetSpec::Local.offset_at(datetime!(2026-01-15 00:00 UTC));
        let _ = UtcOffsetSpec::Local.offset_at(datetime!(2026-08-15 00:00 UTC));
        let _ = UtcOffsetSpec::Local.offset_at(OffsetDateTime::now_utc());
    }
}
