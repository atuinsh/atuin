//! Duration construction and formatting.

use core::fmt;
use std::ops::ControlFlow;

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

    /// Create a `Duration` from a count of nanoseconds, clamping negatives to zero.
    ///
    /// A negative duration is not representable, and in practice means the clock moved
    /// backwards between the two measurements. Zero is the honest answer.
    fn saturating_from_nanos_i64(nanos: i64) -> D;
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

    fn saturating_from_nanos_i64(nanos: i64) -> std::time::Duration {
        std::time::Duration::from_nanos(nanos.max(0).cast_unsigned())
    }
}

impl DurationExt<time::Duration> for time::Duration {
    fn try_new(secs: u64, nsecs: u64) -> Result<time::Duration, DurationOverflow> {
        let std = std::time::Duration::try_new(secs, nsecs)?;
        time::Duration::try_from(std).map_err(|_| DurationOverflow { secs, nsecs })
    }

    fn saturating_from_nanos_i64(nanos: i64) -> time::Duration {
        time::Duration::nanoseconds(nanos.max(0))
    }
}

/// Write `dur` as its most significant unit only, e.g. `1s`, `3d`, `814ms`, `0s`.
///
/// Deliberately lossy: this is sized for narrow, right-aligned terminal columns where a
/// full breakdown would not fit.
#[allow(clippy::module_name_repetitions)]
pub fn format_duration_into(dur: std::time::Duration, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fn item(unit: &'static str, value: u64) -> ControlFlow<(&'static str, u64)> {
        if value > 0 {
            ControlFlow::Break((unit, value))
        } else {
            ControlFlow::Continue(())
        }
    }

    // impl taken and modified from
    // https://github.com/tailhook/humantime/blob/master/src/duration.rs#L295-L331
    // Copyright (c) 2016 The humantime Developers
    fn fmt(f: std::time::Duration) -> ControlFlow<(&'static str, u64), ()> {
        let secs = f.as_secs();
        let nanos = f.subsec_nanos();

        let years = secs / 31_557_600; // 365.25d
        let year_days = secs % 31_557_600;
        let months = year_days / 2_630_016; // 30.44d
        let month_days = year_days % 2_630_016;
        let days = month_days / 86400;
        let day_secs = month_days % 86400;
        let hours = day_secs / 3600;
        let minutes = day_secs % 3600 / 60;
        let seconds = day_secs % 60;

        let millis = nanos / 1_000_000;
        let micros = nanos / 1_000;

        // a difference from our impl than the original is that
        // we only care about the most-significant segment of the duration.
        // If the item call returns `Break`, then the `?` will early-return.
        // This allows for a very consise impl
        item("y", years)?;
        item("mo", months)?;
        item("d", days)?;
        item("h", hours)?;
        item("m", minutes)?;
        item("s", seconds)?;
        item("ms", u64::from(millis))?;
        item("us", u64::from(micros))?;
        item("ns", u64::from(nanos))?;
        ControlFlow::Continue(())
    }

    match fmt(dur) {
        ControlFlow::Break((unit, value)) => write!(f, "{value}{unit}"),
        ControlFlow::Continue(()) => write!(f, "0s"),
    }
}

/// [`format_duration_into`], as an owned `String`.
#[allow(clippy::module_name_repetitions)]
pub fn format_duration(f: std::time::Duration) -> String {
    struct F(std::time::Duration);
    impl fmt::Display for F {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            format_duration_into(self.0, f)
        }
    }
    F(f).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    #[rstest]
    #[case::zero(0, 0)]
    #[case::positive(1_500_000_000, 1_500_000_000)]
    #[case::negative_clamps_to_zero(-1, 0)]
    #[case::min_clamps_to_zero(i64::MIN, 0)]
    #[case::max(i64::MAX, i64::MAX as u128)]
    fn saturating_from_nanos_i64_clamps(#[case] nanos: i64, #[case] expected: u128) {
        assert_eq!(
            std::time::Duration::saturating_from_nanos_i64(nanos).as_nanos(),
            expected
        );
        assert_eq!(
            <time::Duration as DurationExt<_>>::saturating_from_nanos_i64(nanos)
                .whole_nanoseconds(),
            expected as i128
        );
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

    #[rstest]
    #[case::zero(0, "0s")]
    #[case::sub_second(814_000_000, "814ms")]
    #[case::seconds(1_500_000_000, "1s")]
    #[case::minutes(90_000_000_000, "1m")]
    fn format_duration_shows_most_significant_unit(#[case] nanos: u64, #[case] expected: &str) {
        assert_eq!(
            format_duration(std::time::Duration::from_nanos(nanos)),
            expected
        );
    }

    proptest! {
        /// `try_new` never panics, and reports the exact total when it succeeds.
        #[test]
        fn std_try_new_is_total(secs in any::<u64>(), nsecs in any::<u64>()) {
            // u128 is wide enough that this oracle cannot itself overflow
            let expected = u128::from(secs) * 1_000_000_000 + u128::from(nsecs);

            match std::time::Duration::try_new(secs, nsecs) {
                Ok(d) => prop_assert_eq!(d.as_nanos(), expected),
                Err(e) => {
                    prop_assert_eq!(e, DurationOverflow { secs, nsecs });
                    // the only failure mode is the carry exceeding u64 seconds
                    prop_assert!(expected / 1_000_000_000 > u128::from(u64::MAX));
                }
            }
        }

        /// The `time::Duration` impl delegates to the `std` one, so it must succeed
        /// on exactly the same inputs, minus those that overflow `i64` seconds.
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
