//! Times in milliseconds since the Unix epoch, written in a time zone.

use core::fmt;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::DateTimeExt as _;

/// What a [`UnixMillisDisplay`] shows of its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixMillisStyle {
    /// `14:02`.
    Clock,
    /// `14:02:30`.
    ClockSeconds,
    /// `2026-11-14 14:02`.
    YmdHm,
}

/// Writes a time in milliseconds since the Unix epoch in a time zone, or `--:--` for a time
/// outside the range of [`Timestamp`].
///
/// It never fails, so that `to_string` and `serde_json`'s `collect_str` never panic on it.
#[derive(Debug, Clone, Copy)]
pub struct UnixMillisDisplay<'a> {
    millis: i64,
    tz: &'a TimeZone,
    style: UnixMillisStyle,
}

impl<'a> UnixMillisDisplay<'a> {
    /// The time of day, written as `14:02`.
    #[must_use]
    pub const fn clock(millis: i64, tz: &'a TimeZone) -> Self {
        Self {
            millis,
            tz,
            style: UnixMillisStyle::Clock,
        }
    }

    /// The time of day to the second, written as `14:02:30`.
    #[must_use]
    pub const fn clock_seconds(millis: i64, tz: &'a TimeZone) -> Self {
        Self {
            millis,
            tz,
            style: UnixMillisStyle::ClockSeconds,
        }
    }

    /// The date and the time of day, written as `2026-11-14 14:02`.
    #[must_use]
    pub const fn ymd_hm(millis: i64, tz: &'a TimeZone) -> Self {
        Self {
            millis,
            tz,
            style: UnixMillisStyle::YmdHm,
        }
    }
}

impl fmt::Display for UnixMillisDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Ok(at) = Timestamp::from_millisecond(self.millis) else {
            return f.write_str("--:--");
        };
        // Every timestamp has a civil date-time in every time zone.
        let at = self.tz.to_datetime(at);
        match self.style {
            UnixMillisStyle::Clock => write!(f, "{:02}:{:02}", at.hour(), at.minute()),
            UnixMillisStyle::ClockSeconds => {
                write!(f, "{:02}:{:02}:{:02}", at.hour(), at.minute(), at.second())
            }
            UnixMillisStyle::YmdHm => write!(f, "{}", at.display().ymd_hm()),
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::{Offset, TimeZone};
    use rstest::rstest;

    use super::*;

    /// 2026-11-14 14:02:30 UTC.
    const AT_14_02: i64 = 1_794_664_950_000;

    fn offset(hours: i8) -> TimeZone {
        TimeZone::fixed(Offset::constant(hours))
    }

    fn east() -> TimeZone {
        TimeZone::fixed(Offset::from_seconds(5 * 3600 + 30 * 60).unwrap())
    }

    #[rstest]
    #[case::utc(AT_14_02, TimeZone::UTC, "14:02", "2026-11-14 14:02")]
    #[case::east(AT_14_02, east(), "19:32", "2026-11-14 19:32")]
    #[case::before_the_epoch(-60_000, TimeZone::UTC, "23:59", "1969-12-31 23:59")]
    #[case::the_last_time(Timestamp::MAX.as_second() * 1000, offset(1), "23:00", "9999-12-30 23:00")]
    #[case::the_first_time(Timestamp::MIN.as_second() * 1000, offset(-1), "00:59", "-9999-01-02 00:59")]
    #[case::the_largest_time(i64::MAX, offset(2), "--:--", "--:--")]
    #[case::the_smallest_time(i64::MIN, offset(-2), "--:--", "--:--")]
    fn times_write_or_fall_back(
        #[case] millis: i64,
        #[case] tz: TimeZone,
        #[case] clock: &str,
        #[case] ymd_hm: &str,
    ) {
        assert_eq!(UnixMillisDisplay::clock(millis, &tz).to_string(), clock);
        assert_eq!(UnixMillisDisplay::ymd_hm(millis, &tz).to_string(), ymd_hm);
    }

    /// A zone with daylight saving time writes each time in the offset in effect at that time.
    #[rstest]
    #[case::winter(1_768_478_400_000, "13:00")]
    #[case::summer(1_784_116_800_000, "14:00")]
    fn times_follow_daylight_saving_time(#[case] millis: i64, #[case] expected: &str) {
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        assert_eq!(UnixMillisDisplay::clock(millis, &berlin).to_string(), expected);
    }

    #[rstest]
    #[case::utc(AT_14_02, TimeZone::UTC, "14:02:30")]
    #[case::the_largest_time(i64::MAX, offset(1), "--:--")]
    fn clock_seconds_show_the_second(
        #[case] millis: i64,
        #[case] tz: TimeZone,
        #[case] expected: &str,
    ) {
        assert_eq!(UnixMillisDisplay::clock_seconds(millis, &tz).to_string(), expected);
    }
}
