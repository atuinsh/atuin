//! Utilities for operating and manipulating time.

use time::{Duration, OffsetDateTime};

/// Returned by [`DurationExt::try_new`] when the requested seconds/nanoseconds cannot be
/// represented by the target `Duration` type.
///
/// [`std::time::Duration::new`] and [`time::Duration::new`] both *panic* in this situation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duration of {secs}s + {nsecs}ns is out of range")]
pub struct DurationOverflow {
    pub secs: u64,
    pub nsecs: u64,
}

/// Extensions to the Duration classes.
pub trait DurationExt<D> {
    /// Create a `Duration` from whole seconds plus a nanosecond offset.
    ///
    /// [`Duration::new`], but returns [`DurationOverflow`] instead of panicking on overflow.
    fn try_new(secs: u64, nsecs: u64) -> Result<D, DurationOverflow>;
}

impl DurationExt<std::time::Duration> for std::time::Duration {
    #[allow(clippy::disallowed_methods)]
    fn try_new(secs: u64, nsecs: u64) -> Result<std::time::Duration, DurationOverflow> {
        let carry = nsecs / 1_000_000_000;
        let nanos = (nsecs % 1_000_000_000) as u32;
        let secs = secs
            .checked_add(carry)
            .ok_or(DurationOverflow { secs, nsecs })?;
        Ok(std::time::Duration::new(secs, nanos))
    }
}

impl DurationExt<time::Duration> for Duration {
    fn try_new(secs: u64, nsecs: u64) -> Result<time::Duration, DurationOverflow> {
        let std = std::time::Duration::try_new(secs, nsecs)?;
        time::Duration::try_from(std).map_err(|_| DurationOverflow { secs, nsecs })
    }
}

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
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[rstest]
    #[case::whole_seconds(5, 0, 5_000_000_000)]
    #[case::carries_up(1, 2_500_000_000, 3_500_000_000)]
    #[case::only_nanos(0, 1, 1)]
    fn try_new_sums_components(#[case] secs: u64, #[case] nsecs: u64, #[case] expected: u128) {
        assert_eq!(
            std::time::Duration::try_new(secs, nsecs)
                .unwrap()
                .as_nanos(),
            expected
        );
        assert_eq!(
            <time::Duration as DurationExt<_>>::try_new(secs, nsecs)
                .unwrap()
                .whole_nanoseconds(),
            expected as i128
        );
    }

    #[rstest]
    #[case::max_u64_seconds(u64::MAX, 0, true)]
    #[case::largest_nanos_without_carry(u64::MAX, 999_999_999, true)]
    #[case::carry_overflows_u64_seconds(u64::MAX, 1_000_000_000, false)]
    fn std_try_new_range(#[case] secs: u64, #[case] nsecs: u64, #[case] representable: bool) {
        assert_eq!(
            std::time::Duration::try_new(secs, nsecs).is_ok(),
            representable
        );
    }

    #[rstest]
    #[case::max_i64_seconds(i64::MAX as u64, 0, true)]
    #[case::one_second_past_i64(i64::MAX as u64 + 1, 0, false)]
    #[case::carry_crosses_i64(i64::MAX as u64, 1_000_000_000, false)]
    #[case::past_u64_seconds(u64::MAX, 0, false)]
    #[case::carry_overflows_u64_seconds(u64::MAX, 1_000_000_000, false)]
    fn time_try_new_range(#[case] secs: u64, #[case] nsecs: u64, #[case] representable: bool) {
        assert_eq!(
            <time::Duration as DurationExt<_>>::try_new(secs, nsecs).is_ok(),
            representable
        );
    }

    proptest! {
        #[test]
        fn from_timespec_is_total(secs in any::<i128>(), nsecs in any::<i128>()) {
            let result = OffsetDateTime::from_timespec(secs, nsecs);

            match expected_nanos(secs, nsecs) {
                None => prop_assert!(result.is_err()),
                Some(nanos) => {
                    let representable = (MIN_UNIX_NANOS..=MAX_UNIX_NANOS).contains(&nanos);
                    prop_assert_eq!(result.is_ok(), representable);
                    if let Ok(t) = result {
                        prop_assert_eq!(t.unix_timestamp_nanos(), nanos);
                    }
                }
            }
        }

        // Deliberately calls the banned function as the differential oracle. The
        // strategy is confined to the range where upstream is sound, so it is a
        // valid reference there even though it is unusable in general.
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

        #[test]
        fn std_try_new_is_total(secs in any::<u64>(), nsecs in any::<u64>()) {
            let expected = u128::from(secs) * 1_000_000_000 + u128::from(nsecs);

            match std::time::Duration::try_new(secs, nsecs) {
                Ok(d) => prop_assert_eq!(d.as_nanos(), expected),
                Err(e) => {
                    prop_assert_eq!(e, DurationOverflow { secs, nsecs });
                    prop_assert!(expected / 1_000_000_000 > u128::from(u64::MAX));
                }
            }
        }

        #[test]
        fn time_try_new_tracks_std_try_new(secs in any::<u64>(), nsecs in any::<u64>()) {
            let std_result = std::time::Duration::try_new(secs, nsecs);
            let time_result = <time::Duration as DurationExt<_>>::try_new(secs, nsecs);

            match (std_result, time_result) {
                (Ok(s), Ok(t)) => prop_assert_eq!(t.whole_nanoseconds(), s.as_nanos() as i128),
                // time is narrower: it rejects what does not fit an i64 of seconds
                (Ok(s), Err(_)) => prop_assert!(s.as_secs() > i64::MAX as u64),
                (Err(_), Err(_)) => {}
                (Err(_), Ok(_)) => prop_assert!(false, "time succeeded where std failed"),
            }
        }
    }
}
