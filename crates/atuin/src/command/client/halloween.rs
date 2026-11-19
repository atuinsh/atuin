//! The Halloween season, when the Halloween easter eggs come out.

use jiff::ToSpan as _;
use jiff::civil::{Date, Weekday};

/// Halloween dates on [`Date`].
pub trait HalloweenExt {
    /// Whether this day is in the Halloween season: from two weeks before October 31 to the end
    /// of Halloween's weekend, if it falls on one.
    fn is_halloween_season(&self) -> bool;
}

impl HalloweenExt for Date {
    fn is_halloween_season(&self) -> bool {
        let halloween = Self::new(self.year(), 10, 31).expect("October has 31 days");
        let weekend_left = match halloween.weekday() {
            Weekday::Friday => 2,
            Weekday::Saturday => 1,
            Weekday::Sunday
            | Weekday::Monday
            | Weekday::Tuesday
            | Weekday::Wednesday
            | Weekday::Thursday => 0,
        };
        (halloween - 14.days()..=halloween + weekend_left.days()).contains(self)
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{Date, date};
    use rstest::rstest;

    use super::HalloweenExt as _;

    #[rstest]
    #[case::two_weeks_before(date(2026, 10, 17), true)]
    #[case::the_day_before_that(date(2026, 10, 16), false)]
    #[case::halloween(date(2026, 10, 31), true)]
    #[case::sunday_after_a_saturday_halloween(date(2026, 11, 1), true)]
    #[case::monday_after_a_saturday_halloween(date(2026, 11, 2), false)]
    #[case::sunday_after_a_friday_halloween(date(2025, 11, 2), true)]
    #[case::day_after_a_sunday_halloween(date(2027, 11, 1), false)]
    #[case::day_after_a_tuesday_halloween(date(2028, 11, 1), false)]
    #[case::spring(date(2026, 4, 1), false)]
    fn halloween_season(#[case] day: Date, #[case] in_season: bool) {
        assert_eq!(day.is_halloween_season(), in_season);
    }
}
