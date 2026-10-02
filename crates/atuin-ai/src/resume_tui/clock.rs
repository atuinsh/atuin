//! When a session happened, the way the picker says it: a few minutes or hours ago while that is
//! the easiest way to read it, then a clock time, then a date.
//!
//! Relative times get vague fast: everything from yesterday reads `1d ago`. So only the last few
//! hours are relative (`now`, `12m`, `3h`); after that it is `14:02` for today, `yest 09:40`,
//! a weekday within the week (`Mon 09:40`), `Sep 27` within the year, and `2025-09-27` before
//! that. Days are the local ones, in `timezone`.

use time::{Date, Duration, OffsetDateTime, UtcOffset};

/// Newer than this is said relatively (`12m`, `3h`).
const RELATIVE: Duration = Duration::hours(6);

/// The widest [`When::short`] ever is (`yest 09:40`, `2025-09-27`): the time column's width.
pub const WIDTH: usize = 10;

/// When something happened, relative to now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum When {
    /// Under a minute ago (or in the future: another host's clock may be ahead).
    Now,
    /// Minutes or hours ago: `12m`, `3h`.
    Ago(String),
    /// A clock time or date: `14:02`, `yest 09:40`, `Mon 09:40`, `Sep 27`, `2025-09-27`.
    At(String),
}

impl When {
    /// `ts` as seen at `now`, with days in `tz`.
    pub fn of(now: OffsetDateTime, ts: OffsetDateTime, tz: UtcOffset) -> Self {
        let ago = now - ts;
        if ago < Duration::minutes(1) {
            return Self::Now;
        }
        if ago < Duration::hours(1) {
            return Self::Ago(format!("{}m", ago.whole_minutes()));
        }
        if ago < RELATIVE {
            return Self::Ago(format!("{}h", ago.whole_hours()));
        }
        let (today, ts) = (now.to_offset(tz).date(), ts.to_offset(tz));
        let clock = format!("{:02}:{:02}", ts.hour(), ts.minute());
        let day = ts.date();
        Self::At(if day == today {
            clock
        } else if Some(day) == today.previous_day() {
            format!("yest {clock}")
        } else if within_week(day, today) {
            format!("{} {clock}", short(day.weekday()))
        } else if day.year() == today.year() {
            format!("{} {}", short(day.month()), day.day())
        } else {
            format!("{}-{:02}-{:02}", day.year(), u8::from(day.month()), day.day())
        })
    }

    /// For the time column: `now`, `12m`, `3h`, `14:02`, … At most [`WIDTH`] columns.
    pub fn short(&self) -> &str {
        match self {
            Self::Now => "now",
            Self::Ago(s) | Self::At(s) => s,
        }
    }

    /// In a sentence: `just now`, `12m ago`, `14:02`, `yest 09:40`, …
    pub fn phrase(&self) -> String {
        match self {
            Self::Now => "just now".to_owned(),
            Self::Ago(s) => format!("{s} ago"),
            Self::At(s) => s.clone(),
        }
    }
}

/// Beside a full date and time, what it doesn't already say: `12m ago` while recent, else
/// `today`, `yesterday` or a weekday within the week; `None` for anything older.
pub fn beside_date(now: OffsetDateTime, ts: OffsetDateTime, tz: UtcOffset) -> Option<String> {
    let when = When::of(now, ts, tz);
    if !matches!(when, When::At(_)) {
        return Some(when.phrase());
    }
    let (today, day) = (now.to_offset(tz).date(), ts.to_offset(tz).date());
    if day == today {
        Some("today".to_owned())
    } else if Some(day) == today.previous_day() {
        Some("yesterday".to_owned())
    } else if within_week(day, today) {
        Some(day.weekday().to_string())
    } else {
        None
    }
}

/// `day` is one of the six days before `today`, so its weekday names it unambiguously.
fn within_week(day: Date, today: Date) -> bool {
    day < today && (today - day) < Duration::days(7)
}

/// The first three letters of a weekday's or month's name: `Mon`, `Sep`.
fn short(name: impl std::fmt::Display) -> String {
    name.to_string().chars().take(3).collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::macros::{datetime, offset};

    use super::*;

    /// A Monday afternoon.
    const NOW: OffsetDateTime = datetime!(2026-09-28 15:30:00 UTC);

    #[rstest]
    #[case::just_now(datetime!(2026-09-28 15:29:31 UTC), "now")]
    #[case::future(datetime!(2026-09-28 15:40:00 UTC), "now")]
    #[case::a_minute(datetime!(2026-09-28 15:29:00 UTC), "1m")]
    #[case::minutes(datetime!(2026-09-28 15:18:00 UTC), "12m")]
    #[case::an_hour(datetime!(2026-09-28 14:30:00 UTC), "1h")]
    #[case::hours(datetime!(2026-09-28 09:31:00 UTC), "5h")]
    #[case::today(datetime!(2026-09-28 09:30:00 UTC), "09:30")]
    #[case::early_today(datetime!(2026-09-28 00:05:00 UTC), "00:05")]
    #[case::yesterday(datetime!(2026-09-27 23:59:00 UTC), "yest 23:59")]
    #[case::yesterday_morning(datetime!(2026-09-27 09:40:00 UTC), "yest 09:40")]
    #[case::this_week(datetime!(2026-09-24 14:02:00 UTC), "Thu 14:02")]
    #[case::six_days(datetime!(2026-09-22 08:00:00 UTC), "Tue 08:00")]
    #[case::a_week(datetime!(2026-09-21 23:00:00 UTC), "Sep 21")]
    #[case::this_year(datetime!(2026-01-02 12:00:00 UTC), "Jan 2")]
    #[case::last_year(datetime!(2025-09-27 12:00:00 UTC), "2025-09-27")]
    fn short(#[case] ts: OffsetDateTime, #[case] want: &str) {
        let when = When::of(NOW, ts, UtcOffset::UTC);
        assert_eq!(when.short(), want);
        assert!(when.short().len() <= WIDTH);
    }

    /// Days are local: 23:30 UTC on Sunday is Monday morning in Sydney.
    #[rstest]
    fn days_are_local() {
        let sydney = offset!(+10);
        let now = datetime!(2026-09-28 05:00:00 UTC);
        let ts = datetime!(2026-09-27 21:30:00 UTC);
        assert_eq!(When::of(now, ts, sydney).short(), "07:30");
        assert_eq!(When::of(now, ts, UtcOffset::UTC).short(), "yest 21:30");
    }

    #[rstest]
    #[case(datetime!(2026-09-28 15:30:00 UTC), "just now")]
    #[case(datetime!(2026-09-28 15:18:00 UTC), "12m ago")]
    #[case(datetime!(2026-09-27 09:40:00 UTC), "yest 09:40")]
    fn phrase(#[case] ts: OffsetDateTime, #[case] want: &str) {
        assert_eq!(When::of(NOW, ts, UtcOffset::UTC).phrase(), want);
    }

    #[rstest]
    #[case(datetime!(2026-09-28 15:18:00 UTC), Some("12m ago"))]
    #[case(datetime!(2026-09-28 08:00:00 UTC), Some("today"))]
    #[case(datetime!(2026-09-27 08:00:00 UTC), Some("yesterday"))]
    #[case(datetime!(2026-09-24 08:00:00 UTC), Some("Thursday"))]
    #[case(datetime!(2026-09-02 08:00:00 UTC), None)]
    fn beside_a_date(#[case] ts: OffsetDateTime, #[case] want: Option<&str>) {
        assert_eq!(beside_date(NOW, ts, UtcOffset::UTC).as_deref(), want);
    }
}
