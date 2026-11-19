//! The configurable time zone spec, and the time zone it resolves to.

use core::fmt;
use std::str::FromStr;

use jiff::tz::{Offset, TimeZone};
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// A user-supplied time zone spec, and the [`TimeZone`] it names.
///
/// Accepts `local`/`l` for the system's zone; `utc`/`0`; an offset from UTC such as `+09:30` or
/// `-2:30`; or an IANA time zone name such as `Europe/Berlin`. The system's zone is UTC when the
/// system names none, as libc reads it. The spec displays as `local`, the zone's IANA name, or
/// its offset when the zone is a fixed offset.
///
/// [`TimeZone`] is foreign, so it cannot implement `FromStr`/`Deserialize` here. This newtype
/// carries those impls for the config file and CLI flags; read the zone with
/// [`Self::time_zone`].
#[derive(Clone, Debug, Eq, PartialEq, DeserializeFromStr, SerializeDisplay)]
pub struct TimeZoneSpec(Named);

/// What a [`TimeZoneSpec`] names.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Named {
    /// The system's zone, looked up when it is first used. The lookup reads the time zone
    /// database, which takes about a millisecond that a shell hook, loading the config before
    /// and after every command, never needs to spend.
    System,
    Zone(TimeZone),
}

impl TimeZoneSpec {
    const fn zone(tz: TimeZone) -> Self {
        Self(Named::Zone(tz))
    }

    /// The zone this spec names.
    #[must_use]
    pub fn time_zone(&self) -> TimeZone {
        match &self.0 {
            Named::System => TimeZone::system(),
            Named::Zone(tz) => tz.clone(),
        }
    }

    /// Parse `<+|->H[H][:M[M][:S[S]]]`, the offset form of a spec.
    fn parse_offset(spec: &str) -> Option<Offset> {
        const UNITS: [(i32, i32); 3] = [(3600, 25), (60, 59), (1, 59)];

        let (sign, rest) = match spec.split_at_checked(1)? {
            ("+", rest) => (1, rest),
            ("-", rest) => (-1, rest),
            _ => return None,
        };

        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() > UNITS.len() {
            return None;
        }

        let mut seconds = 0;
        for (part, (scale, max)) in parts.into_iter().zip(UNITS) {
            if part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let value: i32 = part.parse().ok()?;
            if value > max {
                return None;
            }
            seconds += value * scale;
        }

        Offset::from_seconds(sign * seconds).ok()
    }
}

impl FromStr for TimeZoneSpec {
    type Err = TimeZoneDecodingError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let lowercase = spec.to_lowercase();

        if matches!(lowercase.as_str(), "l" | "local") {
            return Ok(Self(Named::System));
        }

        if matches!(lowercase.as_str(), "0" | "utc") {
            return Ok(Self::zone(TimeZone::UTC));
        }

        if spec.starts_with(['+', '-']) {
            return Self::parse_offset(spec)
                .map(|offset| Self::zone(TimeZone::fixed(offset)))
                .ok_or_else(|| TimeZoneDecodingError::InvalidOffset(spec.to_owned()));
        }

        TimeZone::get(spec).map(Self::zone).map_err(|source| TimeZoneDecodingError::UnknownZone {
            spec: spec.to_owned(),
            source,
        })
    }
}

impl fmt::Display for TimeZoneSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Named::Zone(tz) = &self.0 else {
            return f.write_str("local");
        };
        if let Some(name) = tz.iana_name() {
            return f.write_str(name);
        }
        match tz.to_fixed_offset() {
            Ok(offset) => write!(f, "{offset}"),
            // Parsing names no zone besides the system's that is neither named nor fixed.
            Err(_) => f.write_str("local"),
        }
    }
}

impl Default for TimeZoneSpec {
    fn default() -> Self {
        Self::zone(TimeZone::UTC)
    }
}

/// Errors returned when parsing a [`TimeZoneSpec`].
#[derive(Debug, thiserror::Error)]
pub enum TimeZoneDecodingError {
    #[error("invalid time zone offset {0:?}: expected the form `+09:30` or `-2`")]
    InvalidOffset(String),
    #[error(
        "unknown time zone {spec:?}: expected `local`, `utc`, an offset such as `+09:30`, or a \
         zone name such as `Europe/Berlin` ({source})"
    )]
    UnknownZone {
        spec: String,
        #[source]
        source: jiff::Error,
    },
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::no_sign("09:30")]
    #[case::garbage("not-a-timezone")]
    #[case::three_digit_hour("+009")]
    #[case::too_many_components("+01:02:03:04")]
    #[case::minutes_out_of_range("+01:60")]
    #[case::hours_out_of_range("+26")]
    #[case::empty_component("+01:")]
    fn rejects_invalid_specs(#[case] spec: &str) {
        assert!(TimeZoneSpec::from_str(spec).is_err());
    }

    #[rstest]
    #[case::utc("utc", 0)]
    #[case::zero("0", 0)]
    #[case::plus("+09:30", 9 * 3600 + 30 * 60)]
    #[case::minus("-2:30", -(2 * 3600 + 30 * 60))]
    #[case::bare_hour("+9", 9 * 3600)]
    #[case::padded_hour("-05", -5 * 3600)]
    #[case::with_seconds("+01:23:45", 3600 + 23 * 60 + 45)]
    // specs are case-insensitive
    #[case::uppercase("UTC", 0)]
    fn resolves_offsets(#[case] spec: &str, #[case] seconds: i32) {
        let spec = TimeZoneSpec::from_str(spec).unwrap();
        assert_eq!(spec.time_zone().to_fixed_offset().unwrap().seconds(), seconds);
    }

    #[rstest]
    #[case::canonical("Europe/Berlin", "Europe/Berlin")]
    #[case::any_case("america/new_york", "America/New_York")]
    fn resolves_zone_names(#[case] spec: &str, #[case] name: &str) {
        assert_eq!(TimeZoneSpec::from_str(spec).unwrap().time_zone().iana_name(), Some(name));
    }

    /// A named zone keeps daylight saving time: Berlin is an hour further from UTC in July.
    #[rstest]
    fn zone_names_follow_daylight_saving_time() {
        let berlin = TimeZoneSpec::from_str("Europe/Berlin").unwrap().time_zone();
        let winter = "2026-01-15T12:00:00Z".parse().unwrap();
        let summer = "2026-07-15T12:00:00Z".parse().unwrap();
        assert_eq!(berlin.to_datetime(winter).hour(), 13);
        assert_eq!(berlin.to_datetime(summer).hour(), 14);
    }

    #[rstest]
    #[case::short("l")]
    #[case::long("local")]
    fn local_names_the_system_zone(#[case] spec: &str) {
        let spec = TimeZoneSpec::from_str(spec).unwrap();
        assert_eq!(spec.time_zone(), TimeZone::system());
        assert_eq!(spec.to_string(), "local");
    }

    #[rstest]
    #[case::utc("utc", "UTC")]
    #[case::offset("+09:30", "+09:30")]
    #[case::offset_with_seconds("-01:23:45", "-01:23:45")]
    #[case::named("europe/berlin", "Europe/Berlin")]
    fn displays_what_it_resolved_to(#[case] spec: &str, #[case] shown: &str) {
        let spec = TimeZoneSpec::from_str(spec).unwrap();
        assert_eq!(spec.to_string(), shown);
        assert_eq!(TimeZoneSpec::from_str(shown).unwrap(), spec);
    }
}
