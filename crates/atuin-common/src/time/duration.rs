//! Duration construction and formatting.

use core::fmt;
use core::marker::PhantomData;
use std::num::NonZeroU64;
use std::ops::ControlFlow;

use easy_cast::Conv;
use serde::de::{self, Visitor};
use serde::{Deserializer, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

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

/// Extensions to the `Duration` types.
pub trait DurationExt<D> {
    /// Create a `Duration` from whole seconds plus a nanosecond offset.
    ///
    /// Like `Duration::new`, but returns [`DurationOverflow`] instead of panicking on overflow.
    fn try_new(secs: u64, nsecs: u64) -> Result<D, DurationOverflow>;

    /// Create a `Duration` from a count of nanoseconds, clamping negatives to zero.
    ///
    /// A negative duration is not representable, and in practice means the clock moved
    /// backwards between the two measurements. Zero is the honest answer.
    fn saturating_from_nanos_i64(nanos: i64) -> D;

    /// Begin rendering this duration.
    ///
    /// Pick a style with [`DurationDisplay::largest_unit`] or [`DurationDisplay::stopwatch`];
    /// the result implements [`Display`](fmt::Display).
    ///
    /// ```ignore
    /// duration.display().stopwatch()     // 1h2m3s
    /// duration.display().largest_unit()  // 1h
    /// ```
    fn display(self) -> DurationDisplay;
}

/// How a [`DurationDisplay`] renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurationStyle {
    /// The largest non-zero unit only: `1s`, `3d`, `814ms`, `0s`.
    #[default]
    LargestUnit,
    /// A stopwatch readout: `1h2m3s`, `1m30s`, `1.234s`, `5ms`.
    ///
    /// Keeps everything down to seconds, and sub-second resolution when that is all
    /// there is. Like a real stopwatch it never rolls past hours, so a three-day
    /// duration reads `72h0m0s`.
    Stopwatch,
}

/// [`Display`](fmt::Display) adapter produced by [`DurationExt::display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationDisplay {
    duration: std::time::Duration,
    style: DurationStyle,
}

impl DurationDisplay {
    /// Render as [`DurationStyle::LargestUnit`].
    #[must_use]
    pub const fn largest_unit(mut self) -> Self {
        self.style = DurationStyle::LargestUnit;
        self
    }

    /// Render as [`DurationStyle::Stopwatch`].
    #[must_use]
    pub const fn stopwatch(mut self) -> Self {
        self.style = DurationStyle::Stopwatch;
        self
    }

    fn fmt_largest_unit(self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn item(unit: &'static str, value: u64) -> ControlFlow<(&'static str, u64)> {
            if value > 0 {
                ControlFlow::Break((unit, value))
            } else {
                ControlFlow::Continue(())
            }
        }

        // impl taken and modified from
        // https://github.com/chronotope/humantime/blob/76c8929b4cc286f675322475a8e1841f35bafc57/src/duration.rs#L427-L465
        // Copyright (c) 2016 The humantime Developers
        fn segments(d: std::time::Duration) -> ControlFlow<(&'static str, u64), ()> {
            let secs = d.as_secs();
            let nanos = d.subsec_nanos();

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

        match segments(self.duration) {
            ControlFlow::Break((unit, value)) => write!(f, "{value}{unit}"),
            ControlFlow::Continue(()) => write!(f, "0s"),
        }
    }

    fn fmt_stopwatch(self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_secs = self.duration.as_secs();
        let millis = self.duration.subsec_millis();

        if total_secs >= 3600 {
            let hours = total_secs / 3600;
            let mins = (total_secs % 3600) / 60;
            let secs = total_secs % 60;
            write!(f, "{hours}h{mins}m{secs}s")
        } else if total_secs >= 60 {
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            write!(f, "{mins}m{secs}s")
        } else if total_secs > 0 {
            if millis > 0 {
                write!(f, "{total_secs}.{millis:03}s")
            } else {
                write!(f, "{total_secs}s")
            }
        } else {
            write!(f, "{millis}ms")
        }
    }
}

impl fmt::Display for DurationDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.style {
            DurationStyle::LargestUnit => self.fmt_largest_unit(f),
            DurationStyle::Stopwatch => self.fmt_stopwatch(f),
        }
    }
}

impl DurationExt<Self> for std::time::Duration {
    #[allow(clippy::disallowed_methods)]
    fn try_new(secs: u64, nsecs: u64) -> Result<Self, DurationOverflow> {
        let carry = nsecs / 1_000_000_000;
        let nanos = u32::conv(nsecs % 1_000_000_000);
        let secs = secs.checked_add(carry).ok_or(DurationOverflow { secs, nsecs })?;
        Ok(Self::new(secs, nanos))
    }

    fn saturating_from_nanos_i64(nanos: i64) -> Self {
        Self::from_nanos(nanos.max(0).cast_unsigned())
    }

    fn display(self) -> DurationDisplay {
        DurationDisplay {
            duration: self,
            style: DurationStyle::default(),
        }
    }
}

impl DurationExt<Self> for time::Duration {
    fn try_new(secs: u64, nsecs: u64) -> Result<Self, DurationOverflow> {
        let std = std::time::Duration::try_new(secs, nsecs)?;
        Self::try_from(std).map_err(|_| DurationOverflow { secs, nsecs })
    }

    fn saturating_from_nanos_i64(nanos: i64) -> Self {
        Self::nanoseconds(nanos.max(0))
    }

    fn display(self) -> DurationDisplay {
        // negative durations are not renderable; clamp rather than invent a sign
        std::time::Duration::try_from(self).unwrap_or_default().display()
    }
}

/// A [`Duration`](std::time::Duration) guaranteed to be non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonZeroDuration(std::time::Duration);

impl NonZeroDuration {
    /// Wrap `duration`, returning `None` if it is zero.
    #[must_use]
    pub const fn new(duration: std::time::Duration) -> Option<Self> {
        if duration.is_zero() {
            None
        } else {
            Some(Self(duration))
        }
    }

    /// Build from a non-zero number of whole seconds.
    #[must_use]
    pub const fn from_secs(secs: NonZeroU64) -> Self {
        Self(std::time::Duration::from_secs(secs.get()))
    }

    /// The wrapped, always-non-zero duration.
    #[must_use]
    pub const fn get(self) -> std::time::Duration {
        self.0
    }
}

/// A time unit used to interpret a bare number in a config duration field.
pub trait DurationUnit {
    /// The number of seconds in one unit.
    const SECS_PER_UNIT: u64;
}

/// A bare number is a count of whole seconds.
pub enum Seconds {}
/// A bare number is a count of whole minutes.
pub enum Minutes {}
/// A bare number is a count of whole days.
pub enum Days {}

impl DurationUnit for Seconds {
    const SECS_PER_UNIT: u64 = 1;
}
impl DurationUnit for Minutes {
    const SECS_PER_UNIT: u64 = 60;
}
impl DurationUnit for Days {
    const SECS_PER_UNIT: u64 = 86_400;
}

/// Parse a duration written as a bare number (interpreted in `U`) or a units string (`"500ms"`,
/// `"5m"`). humantime units are absolute; a unit-less number string is interpreted in `U`.
fn parse_duration_in<'de, U, D>(deserializer: D) -> Result<std::time::Duration, D::Error>
where
    U: DurationUnit,
    D: Deserializer<'de>,
{
    struct DurationVisitor<U>(PhantomData<U>);

    impl<U: DurationUnit> Visitor<'_> for DurationVisitor<U> {
        type Value = std::time::Duration;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a number, or a duration string like \"500ms\"")
        }

        fn visit_u64<E: de::Error>(self, n: u64) -> Result<Self::Value, E> {
            n.checked_mul(U::SECS_PER_UNIT)
                .map(std::time::Duration::from_secs)
                .ok_or_else(|| E::custom("duration is too large"))
        }

        fn visit_i64<E: de::Error>(self, n: i64) -> Result<Self::Value, E> {
            let n = u64::try_from(n)
                .map_err(|_| E::custom(format!("duration cannot be negative: {n}")))?;
            self.visit_u64(n)
        }

        fn visit_f64<E: de::Error>(self, n: f64) -> Result<Self::Value, E> {
            if !n.is_finite() || n < 0.0 {
                return Err(E::custom(format!("invalid duration: {n}")));
            }
            std::time::Duration::try_from_secs_f64(n * U::SECS_PER_UNIT as f64)
                .map_err(|_| E::custom("duration is out of range"))
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            if let Ok(duration) = humantime::parse_duration(value) {
                return Ok(duration);
            }
            if let Ok(n) = value.parse::<u64>() {
                return self.visit_u64(n);
            }
            if let Ok(n) = value.parse::<f64>() {
                return self.visit_f64(n);
            }
            Err(E::custom(format!("invalid duration: {value:?}")))
        }
    }

    deserializer.deserialize_any(DurationVisitor::<U>(PhantomData))
}

/// Render a duration as a units string that round-trips through the duration deserializers.
fn duration_to_string(duration: std::time::Duration) -> String {
    humantime::format_duration(duration).to_string()
}

impl serde::Serialize for NonZeroDuration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&duration_to_string(self.0))
    }
}

impl<'de> serde::Deserialize<'de> for NonZeroDuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(parse_duration_in::<Seconds, D>(deserializer)?)
            .ok_or_else(|| de::Error::custom("duration must be non-zero"))
    }
}

/// `serde_with` marker for a duration whose bare-number unit is `U` (e.g. [`Seconds`], [`Minutes`],
/// [`Days`]), also accepting units strings like `"500ms"`; serializes as a units string.
///
/// Targets both [`std::time::Duration`] (zero allowed) and [`NonZeroDuration`] (zero rejected). Use
/// as `#[serde_as(as = "AsDuration<Minutes>")]`.
pub struct AsDuration<U>(PhantomData<U>);

impl<'de, U: DurationUnit> DeserializeAs<'de, std::time::Duration> for AsDuration<U> {
    fn deserialize_as<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<std::time::Duration, D::Error> {
        parse_duration_in::<U, D>(deserializer)
    }
}

impl<'de, U: DurationUnit> DeserializeAs<'de, NonZeroDuration> for AsDuration<U> {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<NonZeroDuration, D::Error> {
        NonZeroDuration::new(parse_duration_in::<U, D>(deserializer)?)
            .ok_or_else(|| de::Error::custom("duration must be non-zero"))
    }
}

impl<U> SerializeAs<std::time::Duration> for AsDuration<U> {
    fn serialize_as<S: Serializer>(
        source: &std::time::Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&duration_to_string(*source))
    }
}

impl<U> SerializeAs<NonZeroDuration> for AsDuration<U> {
    fn serialize_as<S: Serializer>(
        source: &NonZeroDuration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&duration_to_string(source.get()))
    }
}

/// `serde_with` marker for `Option<NonZeroDuration>` where a zero value means `None` ("disabled").
///
/// Bare numbers use unit `U`; units strings are also accepted. Serializes `None` as `"0"`. Use as
/// `#[serde_as(as = "AsDisableableDuration<Seconds>")]`.
pub struct AsDisableableDuration<U>(PhantomData<U>);

impl<'de, U: DurationUnit> DeserializeAs<'de, Option<NonZeroDuration>>
    for AsDisableableDuration<U>
{
    fn deserialize_as<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<NonZeroDuration>, D::Error> {
        Ok(NonZeroDuration::new(parse_duration_in::<U, D>(deserializer)?))
    }
}

impl<U> SerializeAs<Option<NonZeroDuration>> for AsDisableableDuration<U> {
    fn serialize_as<S: Serializer>(
        source: &Option<NonZeroDuration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match source {
            Some(duration) => serializer.serialize_str(&duration_to_string(duration.get())),
            None => serializer.serialize_str("0"),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[test]
    fn nonzero_duration_rejects_zero() {
        assert!(NonZeroDuration::new(std::time::Duration::ZERO).is_none());
        assert_eq!(
            NonZeroDuration::new(std::time::Duration::from_secs(5)).map(NonZeroDuration::get),
            Some(std::time::Duration::from_secs(5)),
        );
    }

    #[test]
    fn disableable_duration_marker_uses_unit_and_zero_is_none() {
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[serde_as]
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            #[serde_as(as = "AsDisableableDuration<Seconds>")]
            secs: Option<NonZeroDuration>,
            #[serde_as(as = "AsDisableableDuration<Minutes>")]
            mins: Option<NonZeroDuration>,
        }

        let of = |json: &str| serde_json::from_str::<Holder>(json).unwrap();
        let d = |ms| NonZeroDuration::new(std::time::Duration::from_millis(ms));

        // bare int uses the field's unit; 0 disables (None)
        let h = of(r#"{"secs":300,"mins":0}"#);
        assert_eq!(h.secs, d(300_000));
        assert_eq!(h.mins, None);
        // minutes field: bare 5 = 5 minutes
        let h = of(r#"{"secs":0,"mins":5}"#);
        assert_eq!(h.secs, None);
        assert_eq!(h.mins, d(300_000));
        // unit strings are absolute regardless of the field's unit
        let h = of(r#"{"secs":"500ms","mins":"90s"}"#);
        assert_eq!(h.secs, d(500));
        assert_eq!(h.mins, d(90_000));

        // serialize round-trips
        let h = Holder {
            secs: d(500),
            mins: d(300_000),
        };
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Holder>(&json).unwrap(), h);
    }

    #[test]
    fn required_duration_marker_uses_unit_and_zero_policy() {
        use serde::Deserialize;
        use serde_with::serde_as;

        #[serde_as]
        #[derive(Deserialize)]
        struct Holder {
            #[serde_as(as = "AsDuration<Days>")]
            retention: std::time::Duration,
            #[serde_as(as = "AsDuration<Seconds>")]
            timeout: NonZeroDuration,
        }

        // days unit; the plain-Duration target allows zero, and accepts unit strings
        let h: Holder = serde_json::from_str(r#"{"retention":2,"timeout":"1500ms"}"#).unwrap();
        assert_eq!(h.retention, std::time::Duration::from_secs(2 * 86_400));
        assert_eq!(h.timeout.get(), std::time::Duration::from_millis(1500));
        let h: Holder = serde_json::from_str(r#"{"retention":0,"timeout":30}"#).unwrap();
        assert_eq!(h.retention, std::time::Duration::ZERO);

        // the NonZeroDuration target rejects zero
        assert!(serde_json::from_str::<Holder>(r#"{"retention":2,"timeout":0}"#).is_err());
    }

    #[test]
    fn nonzero_duration_deserialize_rejects_zero() {
        assert!(serde_json::from_str::<NonZeroDuration>("0").is_err());
        assert_eq!(
            serde_json::from_str::<NonZeroDuration>(r#""5m""#).unwrap().get(),
            std::time::Duration::from_secs(300),
        );
        assert_eq!(
            serde_json::from_str::<NonZeroDuration>("300").unwrap().get(),
            std::time::Duration::from_secs(300),
        );
    }

    #[rstest]
    #[case::zero(0, 0)]
    #[case::positive(1_500_000_000, 1_500_000_000)]
    #[case::negative_clamps_to_zero(-1, 0)]
    #[case::min_clamps_to_zero(i64::MIN, 0)]
    #[case::max(i64::MAX, u128::conv(i64::MAX))]
    fn saturating_from_nanos_i64_clamps(#[case] nanos: i64, #[case] expected: u128) {
        assert_eq!(std::time::Duration::saturating_from_nanos_i64(nanos).as_nanos(), expected);
        assert_eq!(
            <time::Duration as DurationExt<_>>::saturating_from_nanos_i64(nanos)
                .whole_nanoseconds(),
            i128::conv(expected)
        );
    }

    #[rstest]
    #[case::whole_seconds(5, 0, 5_000_000_000)]
    #[case::carries_up(1, 2_500_000_000, 3_500_000_000)]
    #[case::only_nanos(0, 1, 1)]
    fn try_new_sums_components(#[case] secs: u64, #[case] nsecs: u64, #[case] expected: u128) {
        assert_eq!(std::time::Duration::try_new(secs, nsecs).unwrap().as_nanos(), expected);
        assert_eq!(
            <time::Duration as DurationExt<_>>::try_new(secs, nsecs).unwrap().whole_nanoseconds(),
            i128::conv(expected)
        );
    }

    #[rstest]
    #[case::max_u64_seconds(u64::MAX, 0, true)]
    #[case::largest_nanos_without_carry(u64::MAX, 999_999_999, true)]
    #[case::carry_overflows_u64_seconds(u64::MAX, 1_000_000_000, false)]
    fn std_try_new_range(#[case] secs: u64, #[case] nsecs: u64, #[case] representable: bool) {
        assert_eq!(std::time::Duration::try_new(secs, nsecs).is_ok(), representable);
    }

    #[rstest]
    #[case::max_i64_seconds(u64::conv(i64::MAX), 0, true)]
    #[case::one_second_past_i64(u64::conv(i64::MAX) + 1, 0, false)]
    #[case::carry_crosses_i64(u64::conv(i64::MAX), 1_000_000_000, false)]
    #[case::past_u64_seconds(u64::MAX, 0, false)]
    #[case::carry_overflows_u64_seconds(u64::MAX, 1_000_000_000, false)]
    fn time_try_new_range(#[case] secs: u64, #[case] nsecs: u64, #[case] representable: bool) {
        assert_eq!(<time::Duration as DurationExt<_>>::try_new(secs, nsecs).is_ok(), representable);
    }

    #[rstest]
    #[case::zero(0, "0s")]
    #[case::sub_second(814_000_000, "814ms")]
    #[case::seconds(1_500_000_000, "1s")]
    #[case::minutes(90_000_000_000, "1m")]
    // truncates rather than rounds
    #[case::truncates_not_rounds(7_199_000_000_000, "1h")]
    fn format_duration_shows_most_significant_unit(#[case] nanos: u64, #[case] expected: &str) {
        assert_eq!(
            std::time::Duration::from_nanos(nanos).display().largest_unit().to_string(),
            expected
        );
    }

    #[rstest]
    #[case::zero(0, "0ms")]
    #[case::millis(5_000_000, "5ms")]
    #[case::sub_second_only(814_000_000, "814ms")]
    #[case::whole_second(1_000_000_000, "1s")]
    // sub-second precision is kept, unlike the largest-unit style
    #[case::fractional_second(1_234_000_000, "1.234s")]
    #[case::minutes(90_000_000_000, "1m30s")]
    #[case::hours(3_723_000_000_000, "1h2m3s")]
    // never rolls past hours, unlike the largest-unit style
    #[case::days_stay_in_hours(259_200_000_000_000, "72h0m0s")]
    fn stopwatch_keeps_subsecond_resolution(#[case] nanos: i64, #[case] expected: &str) {
        assert_eq!(
            std::time::Duration::saturating_from_nanos_i64(nanos).display().stopwatch().to_string(),
            expected
        );
    }

    #[test]
    fn stopwatch_clamps_a_negative_time_duration() {
        let negative = time::Duration::nanoseconds(-5_000_000_000);
        assert_eq!(negative.display().stopwatch().to_string(), "0ms");
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
                (Ok(s), Ok(t)) => {
                    prop_assert_eq!(t.whole_nanoseconds(), i128::conv(s.as_nanos()));
                }
                // time is narrower: it rejects what does not fit an i64 of seconds
                (Ok(s), Err(_)) => prop_assert!(s.as_secs() > u64::conv(i64::MAX)),
                (Err(_), Err(_)) => {}
                (Err(_), Ok(_)) => prop_assert!(false, "time succeeded where std failed"),
            }
        }
    }
}
