//! Conversions to and from [`OffsetDateTime`], and the formats we render them with.

use core::fmt;

use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

/// Lowest `time::OffsetDateTime` can represent (unix) `-9999-01-01 00:00:00 UTC`.
const MIN_UNIX_NANOS: i128 = -377_705_116_800 * 1_000_000_000;
/// Highest `time::OffsetDateTime` can represent (unix)`9999-12-31 23:59:59.999_999_999 UTC`.
const MAX_UNIX_NANOS: i128 = 253_402_300_799 * 1_000_000_000 + 999_999_999;

/// Returned when an instant cannot be represented by an [`OffsetDateTime`].
///
/// [`OffsetDateTime::from_unix_timestamp_nanos`] reports some of these as a *successful* conversion
/// to the wrong date -- see <https://github.com/time-rs/time/issues/802>.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("timestamp of {nanos}ns since the unix epoch is out of range")]
pub struct TimestampOutOfRange {
    pub nanos: i128,
}

/// Returned when a seconds/nanoseconds pair cannot be represented by an [`OffsetDateTime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("timespec of {secs}s + {nsecs}ns is out of range")]
pub struct TimespecOutOfRange {
    pub secs: i128,
    pub nsecs: i128,
}

/// Utilities for operating on [`OffsetDateTime`]s.
pub trait OffsetDateTimeExt {
    /// Build an [`OffsetDateTime`] from nanoseconds since the unix epoch.
    fn from_unix_nanos(nanos: i128) -> Result<OffsetDateTime, TimestampOutOfRange>;

    /// Build an [`OffsetDateTime`] from an `i64` count of nanoseconds since the unix epoch.
    fn from_unix_nanos_i64(nanos: i64) -> OffsetDateTime;

    /// Build an [`OffsetDateTime`] from a `u64` count of nanoseconds since the unix epoch.
    fn from_unix_nanos_u64(nanos: u64) -> OffsetDateTime;

    /// Build an [`OffsetDateTime`] from a seconds/nanoseconds pair counted from the unix epoch.
    fn from_timespec(secs: i128, nsecs: i128) -> Result<OffsetDateTime, TimespecOutOfRange>;

    /// Nanoseconds since the unix epoch as an `i64`. Fails outside roughly 1677..2262 AD.
    fn try_unix_nanos_i64(self) -> Result<i64, TimestampOutOfRange>;

    /// Nanoseconds since the unix epoch as a `u64`. Fails before 1970 and after roughly 2554 AD.
    fn try_unix_nanos_u64(self) -> Result<u64, TimestampOutOfRange>;

    /// How much time has passed since `earlier`, clamped to zero if it is in the future.
    fn saturating_duration_since(self, earlier: OffsetDateTime) -> std::time::Duration;

    /// Begin rendering this instant.
    ///
    /// Pick a style with [`OffsetDateTimeDisplay::ymd_hms`] or
    /// [`OffsetDateTimeDisplay::ymd_hm`]; the result implements
    /// [`Display`](fmt::Display).
    ///
    /// ```ignore
    /// datetime.display().ymd_hms()  // 2024-01-22 14:35:07
    /// datetime.display().ymd_hm()   // 2024-01-22 14:35
    /// ```
    fn display(self) -> OffsetDateTimeDisplay;
}

/// How an [`OffsetDateTimeDisplay`] renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffsetDateTimeStyle {
    /// `2024-01-22 14:35:07` -- the default rendering for a history timestamp.
    #[default]
    YmdHms,
    /// `2024-01-22 14:35` -- the same, without seconds, for width-constrained columns.
    YmdHm,
}

/// [`Display`](fmt::Display) adapter produced by [`OffsetDateTimeExt::display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetDateTimeDisplay {
    datetime: OffsetDateTime,
    style: OffsetDateTimeStyle,
}

impl OffsetDateTimeDisplay {
    /// Render as [`OffsetDateTimeStyle::YmdHms`].
    #[must_use]
    pub const fn ymd_hms(mut self) -> Self {
        self.style = OffsetDateTimeStyle::YmdHms;
        self
    }

    /// Render as [`OffsetDateTimeStyle::YmdHm`].
    #[must_use]
    pub const fn ymd_hm(mut self) -> Self {
        self.style = OffsetDateTimeStyle::YmdHm;
        self
    }
}

impl fmt::Display for OffsetDateTimeDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Written component-by-component rather than via `OffsetDateTime::format`, which
        // returns a `Result` that cannot actually fail here. Threading that back through
        // `Display` would only relocate the panic: `ToString::to_string` expects on an
        // `Err`. This way there is nothing to fail.
        let t = self.datetime;

        // `{:04}` counts the sign toward the width, so a negative year has to be padded
        // by hand: year -44 is `-0044`, not `-044`.
        let year = t.year();
        if year < 0 {
            write!(f, "-{:04}", -year)?;
        } else {
            write!(f, "{year:04}")?;
        }

        write!(
            f,
            "-{:02}-{:02} {:02}:{:02}",
            t.month() as u8,
            t.day(),
            t.hour(),
            t.minute()
        )?;

        match self.style {
            OffsetDateTimeStyle::YmdHms => write!(f, ":{:02}", t.second()),
            OffsetDateTimeStyle::YmdHm => Ok(()),
        }
    }
}

const _: () = assert!(
    (i64::MIN as i128) >= MIN_UNIX_NANOS
        && (i64::MAX as i128) <= MAX_UNIX_NANOS
        && (u64::MAX as i128) <= MAX_UNIX_NANOS,
    "the full i64/u64 nanosecond range must be representable as an OffsetDateTime"
);

impl OffsetDateTimeExt for OffsetDateTime {
    #[allow(clippy::disallowed_methods)]
    fn from_unix_nanos(nanos: i128) -> Result<OffsetDateTime, TimestampOutOfRange> {
        if !(MIN_UNIX_NANOS..=MAX_UNIX_NANOS).contains(&nanos) {
            return Err(TimestampOutOfRange { nanos });
        }

        OffsetDateTime::from_unix_timestamp_nanos(nanos).map_err(|_| TimestampOutOfRange { nanos })
    }

    fn from_unix_nanos_i64(nanos: i64) -> OffsetDateTime {
        Self::from_unix_nanos(i128::from(nanos))
            .expect("the full i64 nanosecond range is representable; asserted at compile time")
    }

    fn from_unix_nanos_u64(nanos: u64) -> OffsetDateTime {
        Self::from_unix_nanos(i128::from(nanos))
            .expect("the full u64 nanosecond range is representable; asserted at compile time")
    }

    fn from_timespec(secs: i128, nsecs: i128) -> Result<OffsetDateTime, TimespecOutOfRange> {
        let out_of_range = || TimespecOutOfRange { secs, nsecs };

        let nanos = secs
            .checked_mul(1_000_000_000)
            .and_then(|n| n.checked_add(nsecs))
            .ok_or_else(out_of_range)?;

        Self::from_unix_nanos(nanos).map_err(|_| out_of_range())
    }

    fn try_unix_nanos_i64(self) -> Result<i64, TimestampOutOfRange> {
        let nanos = self.unix_timestamp_nanos();
        i64::try_from(nanos).map_err(|_| TimestampOutOfRange { nanos })
    }

    fn try_unix_nanos_u64(self) -> Result<u64, TimestampOutOfRange> {
        let nanos = self.unix_timestamp_nanos();
        u64::try_from(nanos).map_err(|_| TimestampOutOfRange { nanos })
    }

    fn saturating_duration_since(self, earlier: OffsetDateTime) -> std::time::Duration {
        std::time::Duration::try_from(self - earlier).unwrap_or_default()
    }

    fn display(self) -> OffsetDateTimeDisplay {
        OffsetDateTimeDisplay {
            datetime: self,
            style: OffsetDateTimeStyle::default(),
        }
    }
}

/// `2024-01-22 14:35:07` -- the default rendering for a history timestamp.
pub static YMD_HMS: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour repr:24]:[minute]:[second]");

/// `2024-01-22 14:35` -- the same, without seconds, for width-constrained columns.
pub static YMD_HM: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour repr:24]:[minute]");

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use proptest::prelude::*;
    use rstest::rstest;

    fn expected_nanos(secs: i128, nsecs: i128) -> Option<i128> {
        secs.checked_mul(1_000_000_000)?.checked_add(nsecs)
    }

    #[rstest]
    #[case::zero(0, 0, 0)]
    #[case::whole_seconds(5, 0, 5_000_000_000)]
    #[case::sub_second(1, 500_000_000, 1_500_000_000)]
    #[case::carries_up(1, 2_500_000_000, 3_500_000_000)]
    #[case::borrows_down(1, -500_000_000, 500_000_000)]
    #[case::before_epoch(-1, 0, -1_000_000_000)]
    fn from_timespec_sums_components(
        #[case] secs: i128,
        #[case] nsecs: i128,
        #[case] expected: i128,
    ) {
        let t = OffsetDateTime::from_timespec(secs, nsecs).expect("in range");
        assert_eq!(t.unix_timestamp_nanos(), expected);
    }

    #[rstest]
    #[case::max_instant(253_402_300_799, 999_999_999, true)]
    #[case::one_nano_past_max(253_402_300_799, 1_000_000_000, false)]
    #[case::one_second_past_max(253_402_300_800, 0, false)]
    #[case::min_instant(-377_705_116_800, 0, true)]
    #[case::one_nano_before_min(-377_705_116_800, -1, false)]
    #[case::one_second_before_min(-377_705_116_801, 0, false)]
    #[case::truncation_point_2_63(9_223_372_036_854_775_808, 0, false)]
    #[case::truncation_point_2_64(18_446_744_073_709_551_616, 0, false)]
    #[case::i128_overflow_high(i128::MAX, 0, false)]
    #[case::i128_overflow_low(i128::MIN, 0, false)]
    #[case::i128_overflow_nsecs(0, i128::MAX, false)]
    fn from_timespec_range(#[case] secs: i128, #[case] nsecs: i128, #[case] representable: bool) {
        assert_eq!(
            OffsetDateTime::from_timespec(secs, nsecs).is_ok(),
            representable
        );
    }

    #[rstest]
    #[case::min(i64::MIN)]
    #[case::negative(-1)]
    #[case::epoch(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max(i64::MAX)]
    fn from_unix_nanos_i64_is_total(#[case] nanos: i64) {
        assert_eq!(
            OffsetDateTime::from_unix_nanos_i64(nanos).unix_timestamp_nanos(),
            i128::from(nanos)
        );
    }

    #[rstest]
    #[case::zero(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max(u64::MAX)]
    fn from_unix_nanos_u64_is_total(#[case] nanos: u64) {
        assert_eq!(
            OffsetDateTime::from_unix_nanos_u64(nanos).unix_timestamp_nanos(),
            i128::from(nanos)
        );
    }

    #[rstest]
    // earlier is in the past: the real elapsed time
    #[case::an_hour(3_600, 0, 3_600)]
    #[case::same_instant(0, 0, 0)]
    // earlier is in the *future* (clock skew): clamps to zero rather than going negative
    #[case::future_clamps_to_zero(0, 3_600, 0)]
    fn saturating_duration_since_clamps(
        #[case] now_secs: i64,
        #[case] earlier_secs: i64,
        #[case] expected_secs: u64,
    ) {
        let now = OffsetDateTime::from_unix_nanos_i64(now_secs * 1_000_000_000);
        let earlier = OffsetDateTime::from_unix_nanos_i64(earlier_secs * 1_000_000_000);
        assert_eq!(
            now.saturating_duration_since(earlier).as_secs(),
            expected_secs
        );
    }

    #[rstest]
    #[case::epoch(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max_i64(i64::MAX)]
    fn try_unix_nanos_i64_round_trips(#[case] nanos: i64) {
        let t = OffsetDateTime::from_unix_nanos_i64(nanos);
        assert_eq!(t.try_unix_nanos_i64(), Ok(nanos));
    }

    #[rstest]
    #[case::epoch(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max_u64(u64::MAX)]
    fn try_unix_nanos_u64_round_trips(#[case] nanos: u64) {
        let t = OffsetDateTime::from_unix_nanos_u64(nanos);
        assert_eq!(t.try_unix_nanos_u64(), Ok(nanos));
    }

    #[test]
    fn try_unix_nanos_u64_rejects_pre_epoch() {
        // the silent `as u64` wrap this replaces turned this into ~2554 AD
        let before_epoch = OffsetDateTime::from_unix_nanos_i64(-1);
        assert!(before_epoch.try_unix_nanos_u64().is_err());
        // ...but it is perfectly fine as an i64
        assert_eq!(before_epoch.try_unix_nanos_i64(), Ok(-1));
    }

    #[test]
    fn datetime_formats_render_as_documented() {
        let t = OffsetDateTime::from_unix_nanos_i64(1_705_934_107_000_000_000);
        assert_eq!(t.display().ymd_hms().to_string(), "2024-01-22 14:35:07");
        assert_eq!(t.display().ymd_hm().to_string(), "2024-01-22 14:35");
    }

    /// The `Display` impl writes components by hand so that it cannot fail. These
    /// descriptors are the specification it has to match -- including the padding of a
    /// negative year, where a naive `{:04}` would render `-044` instead of `-0044`.
    #[rstest]
    #[case::epoch(OffsetDateTime::UNIX_EPOCH)]
    #[case::single_digit_components(datetime!(2024-02-06 04:05:06 UTC))]
    #[case::year_one(datetime!(0001-01-01 00:00:00 UTC))]
    #[case::negative_year(datetime!(-0044-03-15 00:00:00 UTC))]
    #[case::max(datetime!(9999-12-31 23:59:59 UTC))]
    fn display_matches_the_format_descriptors(#[case] t: OffsetDateTime) {
        assert_eq!(t.display().ymd_hms().to_string(), t.format(YMD_HMS).unwrap());
        assert_eq!(t.display().ymd_hm().to_string(), t.format(YMD_HM).unwrap());
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn from_timespec_guards_a_hole_upstream_still_has() {
        let two_pow_64: i128 = 1 << 64;
        assert!(
            OffsetDateTime::from_unix_timestamp_nanos(two_pow_64 * 1_000_000_000).is_ok(),
            "precondition: upstream still wraps here"
        );
        assert!(OffsetDateTime::from_timespec(two_pow_64, 0).is_err());
    }

    proptest! {
        /// `from_timespec` accepts exactly the representable instants, and never
        /// panics -- for *any* pair of `i128`s, however absurd.
        #[test]
        fn from_timespec_is_total(secs in any::<i128>(), nsecs in any::<i128>()) {
            let result = OffsetDateTime::from_timespec(secs, nsecs);

            match expected_nanos(secs, nsecs) {
                // combining overflows i128, so it cannot possibly be representable
                None => prop_assert!(result.is_err()),
                Some(nanos) => {
                    let representable = (MIN_UNIX_NANOS..=MAX_UNIX_NANOS).contains(&nanos);
                    prop_assert_eq!(result.is_ok(), representable);
                    // when accepted, the instant is exactly the value asked for
                    if let Ok(t) = result {
                        prop_assert_eq!(t.unix_timestamp_nanos(), nanos);
                    }
                }
            }
        }

        /// Where upstream is sound (no `i64` truncation), we agree with it exactly.
        /// This pins the helper to `time`'s own semantics rather than just to itself.
        #[allow(clippy::disallowed_methods)]
        #[test]
        fn from_timespec_agrees_with_upstream_in_the_sound_range(
            secs in -400_000_000_000i128..=400_000_000_000,
            nsecs in -2_000_000_000i128..=2_000_000_000,
        ) {
            let combined = expected_nanos(secs, nsecs).expect("cannot overflow in this range");
            prop_assert_eq!(
                OffsetDateTime::from_timespec(secs, nsecs).ok(),
                OffsetDateTime::from_unix_timestamp_nanos(combined).ok()
            );
        }
    }
}
