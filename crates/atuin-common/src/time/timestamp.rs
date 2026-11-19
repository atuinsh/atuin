//! Conversions to and from [`Timestamp`].

use jiff::{SignedDuration, Timestamp};

/// Returned when a seconds/nanoseconds pair cannot be represented by a [`Timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("timespec of {secs}s + {nsecs}ns is out of range")]
pub struct TimespecOutOfRange {
    pub secs: i128,
    pub nsecs: i128,
}

/// Utilities for operating on [`Timestamp`]s.
pub trait TimestampExt {
    /// Build a [`Timestamp`] from an `i64` count of nanoseconds since the unix epoch.
    fn from_unix_nanos_i64(nanos: i64) -> Timestamp;

    /// Build a [`Timestamp`] from a `u64` count of nanoseconds since the unix epoch.
    fn from_unix_nanos_u64(nanos: u64) -> Timestamp;

    /// Build a [`Timestamp`] from a seconds/nanoseconds pair counted from the unix epoch.
    fn from_timespec(secs: i128, nsecs: i128) -> Result<Timestamp, TimespecOutOfRange>;

    /// Build a [`Timestamp`] from fractional seconds since the unix epoch, as Python's
    /// `time.time()` gives them, `None` when `secs` is not finite or out of range.
    fn from_unix_secs_f64(secs: f64) -> Option<Timestamp>;

    /// This instant as fractional seconds since the unix epoch, as Python's `time.time()` gives
    /// them.
    ///
    /// An `f64` keeps about 16 significant digits, so a time this century keeps sub-microsecond
    /// precision.
    fn to_unix_secs_f64(self) -> f64;

    /// This instant as a seconds/nanoseconds pair counted from the unix epoch.
    ///
    /// The nanoseconds are always in `0..1_000_000_000`, as protobuf's `Timestamp` requires, so
    /// half a second before the epoch is `(-1, 500_000_000)`.
    fn to_timespec(self) -> (i64, i32);

    /// How much time has passed since `earlier`, clamped to zero if it is in the future.
    fn saturating_duration_since(self, earlier: Timestamp) -> std::time::Duration;
}

/// `-009999-01-02T01:59:59Z`, [`Timestamp::MIN`] in nanoseconds.
const MIN_UNIX_NANOS: i128 = -377_705_023_201 * 1_000_000_000;
/// `9999-12-30T22:00:00.999999999Z`, [`Timestamp::MAX`] in nanoseconds.
const MAX_UNIX_NANOS: i128 = 253_402_207_200 * 1_000_000_000 + 999_999_999;

const _: () = assert!(
    // Using plain `as` casts here because trait methods like `try_from` aren't available in const
    // contexts.
    (i64::MIN as i128) >= MIN_UNIX_NANOS
        && (i64::MAX as i128) <= MAX_UNIX_NANOS
        && (u64::MAX as i128) <= MAX_UNIX_NANOS,
    "the full i64/u64 nanosecond range must be representable as a Timestamp"
);

impl TimestampExt for Timestamp {
    fn from_unix_nanos_i64(nanos: i64) -> Timestamp {
        Self::from_nanosecond(i128::from(nanos))
            .expect("the full i64 nanosecond range is representable; asserted at compile time")
    }

    fn from_unix_nanos_u64(nanos: u64) -> Timestamp {
        Self::from_nanosecond(i128::from(nanos))
            .expect("the full u64 nanosecond range is representable; asserted at compile time")
    }

    fn from_timespec(secs: i128, nsecs: i128) -> Result<Timestamp, TimespecOutOfRange> {
        let out_of_range = || TimespecOutOfRange { secs, nsecs };

        let nanos = secs
            .checked_mul(1_000_000_000)
            .and_then(|n| n.checked_add(nsecs))
            .ok_or_else(out_of_range)?;

        Self::from_nanosecond(nanos).map_err(|_| out_of_range())
    }

    fn from_unix_secs_f64(secs: f64) -> Option<Timestamp> {
        Self::from_duration(SignedDuration::try_from_secs_f64(secs).ok()?).ok()
    }

    fn to_unix_secs_f64(self) -> f64 {
        self.as_duration().as_secs_f64()
    }

    fn to_timespec(self) -> (i64, i32) {
        // jiff truncates toward zero, so the sub-second part of an instant before the epoch is
        // negative. Neither component can overflow: the range is far inside `i64` seconds.
        match (self.as_second(), self.subsec_nanosecond()) {
            (secs, nanos) if nanos < 0 => (secs - 1, nanos + 1_000_000_000),
            timespec => timespec,
        }
    }

    fn saturating_duration_since(self, earlier: Timestamp) -> std::time::Duration {
        std::time::Duration::try_from(self.duration_since(earlier)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn expected_nanos(secs: i128, nsecs: i128) -> Option<i128> {
        secs.checked_mul(1_000_000_000)?.checked_add(nsecs)
    }

    #[rstest]
    fn bounds_match_upstream() {
        assert_eq!(Timestamp::MIN.as_nanosecond(), MIN_UNIX_NANOS);
        assert_eq!(Timestamp::MAX.as_nanosecond(), MAX_UNIX_NANOS);
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
        let t = Timestamp::from_timespec(secs, nsecs).expect("in range");
        assert_eq!(t.as_nanosecond(), expected);
    }

    #[rstest]
    #[case::max_instant(253_402_207_200, 999_999_999, true)]
    #[case::one_nano_past_max(253_402_207_200, 1_000_000_000, false)]
    #[case::one_second_past_max(253_402_207_201, 0, false)]
    #[case::min_instant(-377_705_023_201, 0, true)]
    #[case::one_nano_before_min(-377_705_023_201, -1, false)]
    #[case::one_second_before_min(-377_705_023_202, 0, false)]
    #[case::truncation_point_2_63(9_223_372_036_854_775_808, 0, false)]
    #[case::truncation_point_2_64(18_446_744_073_709_551_616, 0, false)]
    #[case::i128_overflow_high(i128::MAX, 0, false)]
    #[case::i128_overflow_low(i128::MIN, 0, false)]
    #[case::i128_overflow_nsecs(0, i128::MAX, false)]
    fn from_timespec_range(#[case] secs: i128, #[case] nsecs: i128, #[case] representable: bool) {
        assert_eq!(Timestamp::from_timespec(secs, nsecs).is_ok(), representable);
    }

    #[rstest]
    #[case::min(i64::MIN)]
    #[case::negative(-1)]
    #[case::epoch(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max(i64::MAX)]
    fn from_unix_nanos_i64_is_total(#[case] nanos: i64) {
        assert_eq!(Timestamp::from_unix_nanos_i64(nanos).as_nanosecond(), i128::from(nanos));
    }

    #[rstest]
    #[case::zero(0)]
    #[case::typical(1_639_162_832_500_000_000)]
    #[case::max(u64::MAX)]
    fn from_unix_nanos_u64_is_total(#[case] nanos: u64) {
        assert_eq!(Timestamp::from_unix_nanos_u64(nanos).as_nanosecond(), i128::from(nanos));
    }

    #[rstest]
    #[case::epoch(0.0, Some(0))]
    #[case::fractional(1.5, Some(1_500_000_000))]
    #[case::before_epoch(-1.5, Some(-1_500_000_000))]
    #[case::typical(1_705_934_107.25, Some(1_705_934_107_250_000_000))]
    #[case::nan(f64::NAN, None)]
    #[case::infinite(f64::INFINITY, None)]
    #[case::past_max(1e12, None)]
    fn from_unix_secs_f64_reads_seconds(#[case] secs: f64, #[case] nanos: Option<i128>) {
        assert_eq!(Timestamp::from_unix_secs_f64(secs).map(Timestamp::as_nanosecond), nanos);
    }

    #[rstest]
    #[case::epoch(0, 0.0)]
    #[case::fractional(1_500_000_000, 1.5)]
    #[case::before_epoch(-1_500_000_000, -1.5)]
    fn to_unix_secs_f64_gives_seconds(#[case] nanos: i64, #[case] secs: f64) {
        assert!((Timestamp::from_unix_nanos_i64(nanos).to_unix_secs_f64() - secs).abs() < 1e-9);
    }

    #[rstest]
    #[case::epoch(0, (0, 0))]
    #[case::after(1_500_000_000, (1, 500_000_000))]
    #[case::half_second_before(-500_000_000, (-1, 500_000_000))]
    #[case::whole_second_before(-1_000_000_000, (-1, 0))]
    #[case::just_before(-1, (-1, 999_999_999))]
    fn to_timespec_keeps_nanos_positive(#[case] nanos: i64, #[case] timespec: (i64, i32)) {
        assert_eq!(Timestamp::from_unix_nanos_i64(nanos).to_timespec(), timespec);
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
        let now = Timestamp::from_unix_nanos_i64(now_secs * 1_000_000_000);
        let earlier = Timestamp::from_unix_nanos_i64(earlier_secs * 1_000_000_000);
        assert_eq!(now.saturating_duration_since(earlier).as_secs(), expected_secs);
    }

    proptest! {
        /// `from_timespec` accepts exactly the representable instants, and never
        /// panics -- for *any* pair of `i128`s, however absurd.
        #[rstest]
        fn from_timespec_is_total(secs in any::<i128>(), nsecs in any::<i128>()) {
            let result = Timestamp::from_timespec(secs, nsecs);

            match expected_nanos(secs, nsecs) {
                // combining overflows i128, so it cannot possibly be representable
                None => prop_assert!(result.is_err()),
                Some(nanos) => {
                    let representable = (MIN_UNIX_NANOS..=MAX_UNIX_NANOS).contains(&nanos);
                    prop_assert_eq!(result.is_ok(), representable);
                    // when accepted, the instant is exactly the value asked for
                    if let Ok(t) = result {
                        prop_assert_eq!(t.as_nanosecond(), nanos);
                    }
                }
            }
        }

        /// `to_timespec` is the inverse of `from_timespec`, and its nanoseconds stay in range.
        #[rstest]
        fn to_timespec_round_trips(
            nanos in Timestamp::MIN.as_nanosecond()..=Timestamp::MAX.as_nanosecond(),
        ) {
            let t = Timestamp::from_nanosecond(nanos).expect("in range");
            let (secs, nsecs) = t.to_timespec();
            prop_assert!((0..1_000_000_000).contains(&nsecs));
            prop_assert_eq!(Timestamp::from_timespec(secs.into(), nsecs.into()), Ok(t));
        }

        /// Whole seconds are exact in an `f64`, across the whole range.
        #[rstest]
        fn whole_unix_secs_round_trip(
            secs in Timestamp::MIN.as_second()..=Timestamp::MAX.as_second(),
        ) {
            let t = Timestamp::from_second(secs).expect("in range");
            prop_assert_eq!(Timestamp::from_unix_secs_f64(t.to_unix_secs_f64()), Some(t));
        }

        /// Where upstream is sound, we agree with it exactly. This pins the helper to
        /// jiff's own semantics rather than just to itself.
        #[rstest]
        fn from_timespec_agrees_with_upstream_in_the_sound_range(
            secs in -400_000_000_000i128..=400_000_000_000,
            nsecs in -2_000_000_000i128..=2_000_000_000,
        ) {
            let combined = expected_nanos(secs, nsecs).expect("cannot overflow in this range");
            prop_assert_eq!(
                Timestamp::from_timespec(secs, nsecs).ok(),
                Timestamp::from_nanosecond(combined).ok()
            );
        }
    }
}
