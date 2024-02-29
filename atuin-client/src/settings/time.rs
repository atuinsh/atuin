use std::{fmt, str::FromStr};

use config::{builder::DefaultState, ConfigBuilder};
use eyre::{bail, Error, Result};
use serde::Deserialize;
use serde_with::DeserializeFromStr;
use time::{format_description::FormatItem, macros::format_description, UtcOffset};

// Settings

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub timezone: Timezone,
}

// Defaults

pub(crate) fn defaults(
    builder: ConfigBuilder<DefaultState>,
) -> Result<ConfigBuilder<DefaultState>> {
    Ok(builder.set_default("timezone", "local")?)
}

/// format: <+|-><hour>[:<minute>[:<second>]]
static OFFSET_FMT: &[FormatItem<'_>] =
    format_description!("[offset_hour sign:mandatory padding:none][optional [:[offset_minute padding:none][optional [:[offset_second padding:none]]]]]");

/// Type wrapper around `time::UtcOffset` to support a wider variety of timezone formats.
///
/// Note that the parsing of this struct needs to be done before starting any
/// multithreaded runtime, otherwise it will fail on most Unix systems.
///
/// See: https://github.com/atuinsh/atuin/pull/1517#discussion_r1447516426
#[derive(Clone, Copy, Debug, Eq, PartialEq, DeserializeFromStr)]
pub struct Timezone(pub UtcOffset);
impl fmt::Display for Timezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Timezone {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // local timezone
        if matches!(s.to_lowercase().as_str(), "l" | "local") {
            let offset = UtcOffset::current_local_offset()?;
            return Ok(Self(offset));
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

        bail!(r#""{s}" is not a valid timezone spec"#)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use eyre::Result;

    use super::Timezone;

    #[test]
    fn can_parse_offset_timezone_spec() -> Result<()> {
        assert_eq!(Timezone::from_str("+02")?.0.as_hms(), (2, 0, 0));
        assert_eq!(Timezone::from_str("-04")?.0.as_hms(), (-4, 0, 0));
        assert_eq!(Timezone::from_str("+05:30")?.0.as_hms(), (5, 30, 0));
        assert_eq!(Timezone::from_str("-09:30")?.0.as_hms(), (-9, -30, 0));

        // single digit hours are allowed
        assert_eq!(Timezone::from_str("+2")?.0.as_hms(), (2, 0, 0));
        assert_eq!(Timezone::from_str("-4")?.0.as_hms(), (-4, 0, 0));
        assert_eq!(Timezone::from_str("+5:30")?.0.as_hms(), (5, 30, 0));
        assert_eq!(Timezone::from_str("-9:30")?.0.as_hms(), (-9, -30, 0));

        // fully qualified form
        assert_eq!(Timezone::from_str("+09:30:00")?.0.as_hms(), (9, 30, 0));
        assert_eq!(Timezone::from_str("-09:30:00")?.0.as_hms(), (-9, -30, 0));

        // these offsets don't really exist but are supported anyway
        assert_eq!(Timezone::from_str("+0:5")?.0.as_hms(), (0, 5, 0));
        assert_eq!(Timezone::from_str("-0:5")?.0.as_hms(), (0, -5, 0));
        assert_eq!(Timezone::from_str("+01:23:45")?.0.as_hms(), (1, 23, 45));
        assert_eq!(Timezone::from_str("-01:23:45")?.0.as_hms(), (-1, -23, -45));

        // require a leading sign for clarity
        assert!(Timezone::from_str("5").is_err());
        assert!(Timezone::from_str("10:30").is_err());

        Ok(())
    }
}
