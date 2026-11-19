//! The formats we render wall-clock date-times with.

use core::fmt;

use jiff::civil::DateTime;

/// Utilities for operating on [`DateTime`]s.
pub trait DateTimeExt {
    /// Begin rendering this date-time.
    ///
    /// Pick a style with [`DateTimeDisplay::ymd_hms`] or [`DateTimeDisplay::ymd_hm`]; the
    /// result implements [`Display`](fmt::Display).
    ///
    /// ```ignore
    /// tz.to_datetime(timestamp).display().ymd_hms()  // 2024-01-22 14:35:07
    /// tz.to_datetime(timestamp).display().ymd_hm()   // 2024-01-22 14:35
    /// ```
    fn display(self) -> DateTimeDisplay;
}

/// How a [`DateTimeDisplay`] renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateTimeStyle {
    /// `2024-01-22 14:35:07` -- the default rendering for a history timestamp.
    #[default]
    YmdHms,
    /// `2024-01-22 14:35` -- the same, without seconds, for width-constrained columns.
    YmdHm,
}

/// [`Display`](fmt::Display) adapter produced by [`DateTimeExt::display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTimeDisplay {
    datetime: DateTime,
    style: DateTimeStyle,
}

impl DateTimeDisplay {
    /// Render as [`DateTimeStyle::YmdHms`].
    #[must_use]
    pub const fn ymd_hms(mut self) -> Self {
        self.style = DateTimeStyle::YmdHms;
        self
    }

    /// Render as [`DateTimeStyle::YmdHm`].
    #[must_use]
    pub const fn ymd_hm(mut self) -> Self {
        self.style = DateTimeStyle::YmdHm;
        self
    }
}

impl fmt::Display for DateTimeDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Written component-by-component rather than via `DateTime::strftime`, whose
        // `Display` returns an error when formatting fails. `ToString::to_string` panics on
        // that error, so a failure would only move elsewhere. This way there is nothing to
        // fail.
        let t = self.datetime;

        // `{:04}` counts the sign toward the width, so a negative year has to be padded
        // by hand: year -44 is `-0044`, not `-044`.
        let year = t.year();
        if year < 0 {
            write!(f, "-{:04}", -year)?;
        } else {
            write!(f, "{year:04}")?;
        }

        write!(f, "-{:02}-{:02} {:02}:{:02}", t.month(), t.day(), t.hour(), t.minute())?;

        match self.style {
            DateTimeStyle::YmdHms => write!(f, ":{:02}", t.second()),
            DateTimeStyle::YmdHm => Ok(()),
        }
    }
}

impl DateTimeExt for DateTime {
    fn display(self) -> DateTimeDisplay {
        DateTimeDisplay {
            datetime: self,
            style: DateTimeStyle::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use rstest::rstest;

    use super::*;

    /// `2024-01-22 14:35:07`, the [`strftime`](DateTime::strftime) spec of
    /// [`DateTimeStyle::YmdHms`].
    const YMD_HMS: &str = "%Y-%m-%d %H:%M:%S";
    /// `2024-01-22 14:35`, the [`strftime`](DateTime::strftime) spec of
    /// [`DateTimeStyle::YmdHm`].
    const YMD_HM: &str = "%Y-%m-%d %H:%M";

    #[rstest]
    #[case::typical(date(2024, 1, 22).at(14, 35, 7, 0), "2024-01-22 14:35:07", "2024-01-22 14:35")]
    // A naive `{:04}`, like strftime's `%Y`, counts the sign toward the width and renders
    // `-044`.
    #[case::negative_year(
        date(-44, 3, 15).at(0, 0, 0, 0),
        "-0044-03-15 00:00:00",
        "-0044-03-15 00:00"
    )]
    fn datetime_formats_render_as_documented(
        #[case] t: DateTime,
        #[case] ymd_hms: &str,
        #[case] ymd_hm: &str,
    ) {
        assert_eq!(t.display().ymd_hms().to_string(), ymd_hms);
        assert_eq!(t.display().ymd_hm().to_string(), ymd_hm);
    }

    /// The `Display` impl writes components by hand so that it cannot fail. Outside negative
    /// years, these strftime specs are what it has to match.
    #[rstest]
    #[case::epoch(date(1970, 1, 1).at(0, 0, 0, 0))]
    #[case::single_digit_components(date(2024, 2, 6).at(4, 5, 6, 0))]
    #[case::year_one(date(1, 1, 1).at(0, 0, 0, 0))]
    #[case::max(date(9999, 12, 31).at(23, 59, 59, 0))]
    fn display_matches_strftime(#[case] t: DateTime) {
        assert_eq!(t.display().ymd_hms().to_string(), t.strftime(YMD_HMS).to_string());
        assert_eq!(t.display().ymd_hm().to_string(), t.strftime(YMD_HM).to_string());
    }
}
