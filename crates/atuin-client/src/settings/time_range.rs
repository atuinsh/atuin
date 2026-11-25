//! A time range that a user gave as two optional ends, such as a command's `--after` and
//! `--before` flags.

use jiff::{Timestamp, ZonedDateTime};

use super::{Dialect, ParseRangeEndError, RangeEnd};

/// One end of a range as the user gave it.
#[derive(Debug, Clone, Copy)]
pub struct GivenEnd<'a> {
    /// The name of the argument that gave the end, which an error names.
    pub name: &'static str,
    pub text: &'a str,
}

/// Describes why the ends of a range could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TimeRangeError {
    #[error("{name} {text:?}: {source}")]
    Time {
        /// The name of the argument that gave the end.
        name: &'static str,
        text: String,
        #[source]
        source: ParseRangeEndError,
    },
    #[error("{from} is later than {to}")]
    Backwards {
        /// The name of the argument that gave the start.
        from: &'static str,
        /// The name of the argument that gave the end.
        to: &'static str,
    },
    #[error("{to} is earlier than {default_from}, where the range starts by default")]
    BeforeDefaultStart {
        /// The name of the argument that gave the end.
        to: &'static str,
        default_from: Timestamp,
    },
}

/// A time range, read from the two ends that a user gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub from: Timestamp,
    /// `None` when the range reaches to now.
    pub to: Option<Timestamp>,
}

impl TimeRange {
    /// Reads `from` and `to` as [`Dialect::parse_range_end`] reads them, with `to` as the end
    /// `to_end`. An absent `from` is `default_from`, and an absent `to` reaches to now.
    ///
    /// # Errors
    ///
    /// Fails when an end can't be read, and when the start of the range, given or by default, is
    /// later than `to`.
    pub fn parse(
        from: Option<GivenEnd<'_>>,
        to: Option<GivenEnd<'_>>,
        to_end: RangeEnd,
        default_from: Timestamp,
        now: &ZonedDateTime,
        dialect: Dialect,
    ) -> Result<Self, TimeRangeError> {
        let parse = |given: GivenEnd<'_>, end| {
            dialect.parse_range_end(given.text, now, end).map_err(|source| TimeRangeError::Time {
                name: given.name,
                text: given.text.to_owned(),
                source,
            })
        };
        let start = from.map(|from| parse(from, RangeEnd::Start)).transpose()?;
        let end = to.map(|to| parse(to, to_end)).transpose()?;
        let range = Self {
            from: start.unwrap_or(default_from),
            to: end,
        };
        if let (Some(to), Some(end)) = (to, end)
            && range.from > end
        {
            return Err(match from {
                Some(from) => TimeRangeError::Backwards {
                    from: from.name,
                    to: to.name,
                },
                None => TimeRangeError::BeforeDefaultStart {
                    to: to.name,
                    default_from,
                },
            });
        }
        Ok(range)
    }

    /// The start of the range, in milliseconds since the Unix epoch.
    #[must_use]
    pub fn start_ms(&self) -> i64 {
        self.from.as_millisecond()
    }

    /// The end of the range in the form that the daemon's requests take, where 0 means now.
    #[must_use]
    pub fn wire_to_ms(&self) -> i64 {
        self.to.map_or(0, Timestamp::as_millisecond)
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;
    use jiff::tz::TimeZone;
    use rstest::rstest;

    use super::*;

    fn noon() -> ZonedDateTime {
        "2026-11-14T12:00:00Z".parse::<Timestamp>().unwrap().to_zoned_datetime(TimeZone::UTC)
    }

    /// A day as the end reaches to the day's end or stops at its start, as `to_end` says, and a
    /// start that is later than now or than the end is refused.
    ///
    /// The range starts 24 hours ago by default.
    #[rstest]
    #[case::a_day_through_its_end(
        Some("yesterday"),
        Some("yesterday"),
        RangeEnd::Through,
        Ok(("2026-11-13T00:00:00Z", Some("2026-11-14T00:00:00Z")))
    )]
    #[case::a_day_before_its_start(
        None,
        Some("today"),
        RangeEnd::Before,
        Ok(("2026-11-13T12:00:00Z", Some("2026-11-14T00:00:00Z")))
    )]
    #[case::open_ended(Some("2h"), None, RangeEnd::Through, Ok(("2026-11-14T10:00:00Z", None)))]
    #[case::a_later_start(
        Some("tomorrow"),
        None,
        RangeEnd::Through,
        Err("from \"tomorrow\": the start of the range is later than now")
    )]
    #[case::an_unknown_time(
        None,
        Some("soon"),
        RangeEnd::Through,
        Err("to \"soon\": give an RFC 3339 time")
    )]
    #[case::backwards(Some("1h"), Some("2h"), RangeEnd::Before, Err("from is later than to"))]
    #[case::an_end_before_the_default_start(
        None,
        Some("3 days ago"),
        RangeEnd::Through,
        Err("to is earlier than 2026-11-13T12:00:00Z, where the range starts by default")
    )]
    fn ranges_parse(
        #[case] from: Option<&'static str>,
        #[case] to: Option<&'static str>,
        #[case] to_end: RangeEnd,
        #[case] expected: Result<(&str, Option<&str>), &str>,
    ) {
        let range = TimeRange::parse(
            from.map(|text| GivenEnd { name: "from", text }),
            to.map(|text| GivenEnd { name: "to", text }),
            to_end,
            noon().timestamp() - SignedDuration::from_hours(24),
            &noon(),
            Dialect::Uk,
        );
        match (range, expected) {
            (Ok(range), Ok((from, to))) => assert_eq!(range, TimeRange {
                from: from.parse().unwrap(),
                to: to.map(|to| to.parse().unwrap()),
            }),
            (Err(err), Err(start)) => assert!(err.to_string().starts_with(start), "{err}"),
            (Ok(range), Err(_)) => panic!("{range:?}"),
            (Err(err), Ok(_)) => panic!("{err}"),
        }
    }

    #[rstest]
    #[case::to_now(None, 0)]
    #[case::to_a_time(Some(1_700_000_000_000), 1_700_000_000_000)]
    fn the_wire_end_is_zero_for_now(#[case] to_ms: Option<i64>, #[case] expected: i64) {
        let range = TimeRange {
            from: Timestamp::UNIX_EPOCH,
            to: to_ms.map(|ms| Timestamp::from_millisecond(ms).unwrap()),
        };
        assert_eq!(range.wire_to_ms(), expected);
    }
}
