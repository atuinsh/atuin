//! The Halloween season, when the Halloween easter eggs come out.

use time::{Date, Duration, Month, Weekday};

/// Halloween dates on [`Date`].
pub trait HalloweenExt {
    /// Whether this day is in the Halloween season: from two weeks before October 31 to the end
    /// of Halloween's weekend, if it falls on one.
    fn is_halloween_season(&self) -> bool;
}

impl HalloweenExt for Date {
    fn is_halloween_season(&self) -> bool {
        let halloween =
            Self::from_calendar_date(self.year(), Month::October, 31).expect("October has 31 days");
        let last_day = match halloween.weekday() {
            Weekday::Friday => halloween + Duration::days(2),
            Weekday::Saturday => halloween + Duration::days(1),
            Weekday::Sunday
            | Weekday::Monday
            | Weekday::Tuesday
            | Weekday::Wednesday
            | Weekday::Thursday => halloween,
        };
        (halloween - Duration::days(14)..=last_day).contains(self)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::Date;
    use time::macros::date;

    use super::HalloweenExt as _;

    #[rstest]
    #[case::two_weeks_before(date!(2026-10-17), true)]
    #[case::the_day_before_that(date!(2026-10-16), false)]
    #[case::halloween(date!(2026-10-31), true)]
    #[case::sunday_after_a_saturday_halloween(date!(2026-11-01), true)]
    #[case::monday_after_a_saturday_halloween(date!(2026-11-02), false)]
    #[case::sunday_after_a_friday_halloween(date!(2025-11-02), true)]
    #[case::day_after_a_sunday_halloween(date!(2027-11-01), false)]
    #[case::day_after_a_tuesday_halloween(date!(2028-11-01), false)]
    #[case::spring(date!(2026-04-01), false)]
    fn halloween_season(#[case] day: Date, #[case] in_season: bool) {
        assert_eq!(day.is_halloween_season(), in_season);
    }
}
