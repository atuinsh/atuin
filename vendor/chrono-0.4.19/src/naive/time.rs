// This is a part of Chrono.
// See README.md and LICENSE.txt for details.

//! ISO 8601 time without timezone.

#[cfg(any(feature = "alloc", feature = "std", test))]
use core::borrow::Borrow;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::{fmt, hash, str};
use oldtime::Duration as OldDuration;

use div::div_mod_floor;
#[cfg(any(feature = "alloc", feature = "std", test))]
use format::DelayedFormat;
use format::{parse, ParseError, ParseResult, Parsed, StrftimeItems};
use format::{Fixed, Item, Numeric, Pad};
use Timelike;

pub const MIN_TIME: NaiveTime = NaiveTime { secs: 0, frac: 0 };
pub const MAX_TIME: NaiveTime = NaiveTime { secs: 23 * 3600 + 59 * 60 + 59, frac: 999_999_999 };

/// ISO 8601 time without timezone.
/// Allows for the nanosecond precision and optional leap second representation.
///
/// # Leap Second Handling
///
/// Since 1960s, the manmade atomic clock has been so accurate that
/// it is much more accurate than Earth's own motion.
/// It became desirable to define the civil time in terms of the atomic clock,
/// but that risks the desynchronization of the civil time from Earth.
/// To account for this, the designers of the Coordinated Universal Time (UTC)
/// made that the UTC should be kept within 0.9 seconds of the observed Earth-bound time.
/// When the mean solar day is longer than the ideal (86,400 seconds),
/// the error slowly accumulates and it is necessary to add a **leap second**
/// to slow the UTC down a bit.
/// (We may also remove a second to speed the UTC up a bit, but it never happened.)
/// The leap second, if any, follows 23:59:59 of June 30 or December 31 in the UTC.
///
/// Fast forward to the 21st century,
/// we have seen 26 leap seconds from January 1972 to December 2015.
/// Yes, 26 seconds. Probably you can read this paragraph within 26 seconds.
/// But those 26 seconds, and possibly more in the future, are never predictable,
/// and whether to add a leap second or not is known only before 6 months.
/// Internet-based clocks (via NTP) do account for known leap seconds,
/// but the system API normally doesn't (and often can't, with no network connection)
/// and there is no reliable way to retrieve leap second information.
///
/// Chrono does not try to accurately implement leap seconds; it is impossible.
/// Rather, **it allows for leap seconds but behaves as if there are *no other* leap seconds.**
/// Various operations will ignore any possible leap second(s)
/// except when any of the operands were actually leap seconds.
///
/// If you cannot tolerate this behavior,
/// you must use a separate `TimeZone` for the International Atomic Time (TAI).
/// TAI is like UTC but has no leap seconds, and thus slightly differs from UTC.
/// Chrono does not yet provide such implementation, but it is planned.
///
/// ## Representing Leap Seconds
///
/// The leap second is indicated via fractional seconds more than 1 second.
/// This makes possible to treat a leap second as the prior non-leap second
/// if you don't care about sub-second accuracy.
/// You should use the proper formatting to get the raw leap second.
///
/// All methods accepting fractional seconds will accept such values.
///
/// ~~~~
/// use chrono::{NaiveDate, NaiveTime, Utc, TimeZone};
///
/// let t = NaiveTime::from_hms_milli(8, 59, 59, 1_000);
///
/// let dt1 = NaiveDate::from_ymd(2015, 7, 1).and_hms_micro(8, 59, 59, 1_000_000);
///
/// let dt2 = Utc.ymd(2015, 6, 30).and_hms_nano(23, 59, 59, 1_000_000_000);
/// # let _ = (t, dt1, dt2);
/// ~~~~
///
/// Note that the leap second can happen anytime given an appropriate time zone;
/// 2015-07-01 01:23:60 would be a proper leap second if UTC+01:24 had existed.
/// Practically speaking, though, by the time of the first leap second on 1972-06-30,
/// every time zone offset around the world has standardized to the 5-minute alignment.
///
/// ## Date And Time Arithmetics
///
/// As a concrete example, let's assume that `03:00:60` and `04:00:60` are leap seconds.
/// In reality, of course, leap seconds are separated by at least 6 months.
/// We will also use some intuitive concise notations for the explanation.
///
/// `Time + Duration`
/// (short for [`NaiveTime::overflowing_add_signed`](#method.overflowing_add_signed)):
///
/// - `03:00:00 + 1s = 03:00:01`.
/// - `03:00:59 + 60s = 03:02:00`.
/// - `03:00:59 + 1s = 03:01:00`.
/// - `03:00:60 + 1s = 03:01:00`.
///   Note that the sum is identical to the previous.
/// - `03:00:60 + 60s = 03:01:59`.
/// - `03:00:60 + 61s = 03:02:00`.
/// - `03:00:60.1 + 0.8s = 03:00:60.9`.
///
/// `Time - Duration`
/// (short for [`NaiveTime::overflowing_sub_signed`](#method.overflowing_sub_signed)):
///
/// - `03:00:00 - 1s = 02:59:59`.
/// - `03:01:00 - 1s = 03:00:59`.
/// - `03:01:00 - 60s = 03:00:00`.
/// - `03:00:60 - 60s = 03:00:00`.
///   Note that the result is identical to the previous.
/// - `03:00:60.7 - 0.4s = 03:00:60.3`.
/// - `03:00:60.7 - 0.9s = 03:00:59.8`.
///
/// `Time - Time`
/// (short for [`NaiveTime::signed_duration_since`](#method.signed_duration_since)):
///
/// - `04:00:00 - 03:00:00 = 3600s`.
/// - `03:01:00 - 03:00:00 = 60s`.
/// - `03:00:60 - 03:00:00 = 60s`.
///   Note that the difference is identical to the previous.
/// - `03:00:60.6 - 03:00:59.4 = 1.2s`.
/// - `03:01:00 - 03:00:59.8 = 0.2s`.
/// - `03:01:00 - 03:00:60.5 = 0.5s`.
///   Note that the difference is larger than the previous,
///   even though the leap second clearly follows the previous whole second.
/// - `04:00:60.9 - 03:00:60.1 =
///   (04:00:60.9 - 04:00:00) + (04:00:00 - 03:01:00) + (03:01:00 - 03:00:60.1) =
///   60.9s + 3540s + 0.9s = 3601.8s`.
///
/// In general,
///
/// - `Time + Duration` unconditionally equals to `Duration + Time`.
///
/// - `Time - Duration` unconditionally equals to `Time + (-Duration)`.
///
/// - `Time1 - Time2` unconditionally equals to `-(Time2 - Time1)`.
///
/// - Associativity does not generally hold, because
///   `(Time + Duration1) - Duration2` no longer equals to `Time + (Duration1 - Duration2)`
///   for two positive durations.
///
///     - As a special case, `(Time + Duration) - Duration` also does not equal to `Time`.
///
///     - If you can assume that all durations have the same sign, however,
///       then the associativity holds:
///       `(Time + Duration1) + Duration2` equals to `Time + (Duration1 + Duration2)`
///       for two positive durations.
///
/// ## Reading And Writing Leap Seconds
///
/// The "typical" leap seconds on the minute boundary are
/// correctly handled both in the formatting and parsing.
/// The leap second in the human-readable representation
/// will be represented as the second part being 60, as required by ISO 8601.
///
/// ~~~~
/// use chrono::{Utc, TimeZone};
///
/// let dt = Utc.ymd(2015, 6, 30).and_hms_milli(23, 59, 59, 1_000);
/// assert_eq!(format!("{:?}", dt), "2015-06-30T23:59:60Z");
/// ~~~~
///
/// There are hypothetical leap seconds not on the minute boundary
/// nevertheless supported by Chrono.
/// They are allowed for the sake of completeness and consistency;
/// there were several "exotic" time zone offsets with fractional minutes prior to UTC after all.
/// For such cases the human-readable representation is ambiguous
/// and would be read back to the next non-leap second.
///
/// ~~~~
/// use chrono::{DateTime, Utc, TimeZone};
///
/// let dt = Utc.ymd(2015, 6, 30).and_hms_milli(23, 56, 4, 1_000);
/// assert_eq!(format!("{:?}", dt), "2015-06-30T23:56:05Z");
///
/// let dt = Utc.ymd(2015, 6, 30).and_hms(23, 56, 5);
/// assert_eq!(format!("{:?}", dt), "2015-06-30T23:56:05Z");
/// assert_eq!(DateTime::parse_from_rfc3339("2015-06-30T23:56:05Z").unwrap(), dt);
/// ~~~~
///
/// Since Chrono alone cannot determine any existence of leap seconds,
/// **there is absolutely no guarantee that the leap second read has actually happened**.
#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub struct NaiveTime {
    secs: u32,
    frac: u32,
}

impl NaiveTime {
    /// Makes a new `NaiveTime` from hour, minute and second.
    ///
    /// No [leap second](#leap-second-handling) is allowed here;
    /// use `NaiveTime::from_hms_*` methods with a subsecond parameter instead.
    ///
    /// Panics on invalid hour, minute and/or second.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let t = NaiveTime::from_hms(23, 56, 4);
    /// assert_eq!(t.hour(), 23);
    /// assert_eq!(t.minute(), 56);
    /// assert_eq!(t.second(), 4);
    /// assert_eq!(t.nanosecond(), 0);
    /// ~~~~
    #[inline]
    pub fn from_hms(hour: u32, min: u32, sec: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, min, sec).expect("invalid time")
    }

    /// Makes a new `NaiveTime` from hour, minute and second.
    ///
    /// No [leap second](#leap-second-handling) is allowed here;
    /// use `NaiveTime::from_hms_*_opt` methods with a subsecond parameter instead.
    ///
    /// Returns `None` on invalid hour, minute and/or second.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let from_hms_opt = NaiveTime::from_hms_opt;
    ///
    /// assert!(from_hms_opt(0, 0, 0).is_some());
    /// assert!(from_hms_opt(23, 59, 59).is_some());
    /// assert!(from_hms_opt(24, 0, 0).is_none());
    /// assert!(from_hms_opt(23, 60, 0).is_none());
    /// assert!(from_hms_opt(23, 59, 60).is_none());
    /// ~~~~
    #[inline]
    pub fn from_hms_opt(hour: u32, min: u32, sec: u32) -> Option<NaiveTime> {
        NaiveTime::from_hms_nano_opt(hour, min, sec, 0)
    }

    /// Makes a new `NaiveTime` from hour, minute, second and millisecond.
    ///
    /// The millisecond part can exceed 1,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Panics on invalid hour, minute, second and/or millisecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let t = NaiveTime::from_hms_milli(23, 56, 4, 12);
    /// assert_eq!(t.hour(), 23);
    /// assert_eq!(t.minute(), 56);
    /// assert_eq!(t.second(), 4);
    /// assert_eq!(t.nanosecond(), 12_000_000);
    /// ~~~~
    #[inline]
    pub fn from_hms_milli(hour: u32, min: u32, sec: u32, milli: u32) -> NaiveTime {
        NaiveTime::from_hms_milli_opt(hour, min, sec, milli).expect("invalid time")
    }

    /// Makes a new `NaiveTime` from hour, minute, second and millisecond.
    ///
    /// The millisecond part can exceed 1,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Returns `None` on invalid hour, minute, second and/or millisecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let from_hmsm_opt = NaiveTime::from_hms_milli_opt;
    ///
    /// assert!(from_hmsm_opt(0, 0, 0, 0).is_some());
    /// assert!(from_hmsm_opt(23, 59, 59, 999).is_some());
    /// assert!(from_hmsm_opt(23, 59, 59, 1_999).is_some()); // a leap second after 23:59:59
    /// assert!(from_hmsm_opt(24, 0, 0, 0).is_none());
    /// assert!(from_hmsm_opt(23, 60, 0, 0).is_none());
    /// assert!(from_hmsm_opt(23, 59, 60, 0).is_none());
    /// assert!(from_hmsm_opt(23, 59, 59, 2_000).is_none());
    /// ~~~~
    #[inline]
    pub fn from_hms_milli_opt(hour: u32, min: u32, sec: u32, milli: u32) -> Option<NaiveTime> {
        milli
            .checked_mul(1_000_000)
            .and_then(|nano| NaiveTime::from_hms_nano_opt(hour, min, sec, nano))
    }

    /// Makes a new `NaiveTime` from hour, minute, second and microsecond.
    ///
    /// The microsecond part can exceed 1,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Panics on invalid hour, minute, second and/or microsecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let t = NaiveTime::from_hms_micro(23, 56, 4, 12_345);
    /// assert_eq!(t.hour(), 23);
    /// assert_eq!(t.minute(), 56);
    /// assert_eq!(t.second(), 4);
    /// assert_eq!(t.nanosecond(), 12_345_000);
    /// ~~~~
    #[inline]
    pub fn from_hms_micro(hour: u32, min: u32, sec: u32, micro: u32) -> NaiveTime {
        NaiveTime::from_hms_micro_opt(hour, min, sec, micro).expect("invalid time")
    }

    /// Makes a new `NaiveTime` from hour, minute, second and microsecond.
    ///
    /// The microsecond part can exceed 1,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Returns `None` on invalid hour, minute, second and/or microsecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let from_hmsu_opt = NaiveTime::from_hms_micro_opt;
    ///
    /// assert!(from_hmsu_opt(0, 0, 0, 0).is_some());
    /// assert!(from_hmsu_opt(23, 59, 59, 999_999).is_some());
    /// assert!(from_hmsu_opt(23, 59, 59, 1_999_999).is_some()); // a leap second after 23:59:59
    /// assert!(from_hmsu_opt(24, 0, 0, 0).is_none());
    /// assert!(from_hmsu_opt(23, 60, 0, 0).is_none());
    /// assert!(from_hmsu_opt(23, 59, 60, 0).is_none());
    /// assert!(from_hmsu_opt(23, 59, 59, 2_000_000).is_none());
    /// ~~~~
    #[inline]
    pub fn from_hms_micro_opt(hour: u32, min: u32, sec: u32, micro: u32) -> Option<NaiveTime> {
        micro.checked_mul(1_000).and_then(|nano| NaiveTime::from_hms_nano_opt(hour, min, sec, nano))
    }

    /// Makes a new `NaiveTime` from hour, minute, second and nanosecond.
    ///
    /// The nanosecond part can exceed 1,000,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Panics on invalid hour, minute, second and/or nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let t = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(t.hour(), 23);
    /// assert_eq!(t.minute(), 56);
    /// assert_eq!(t.second(), 4);
    /// assert_eq!(t.nanosecond(), 12_345_678);
    /// ~~~~
    #[inline]
    pub fn from_hms_nano(hour: u32, min: u32, sec: u32, nano: u32) -> NaiveTime {
        NaiveTime::from_hms_nano_opt(hour, min, sec, nano).expect("invalid time")
    }

    /// Makes a new `NaiveTime` from hour, minute, second and nanosecond.
    ///
    /// The nanosecond part can exceed 1,000,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Returns `None` on invalid hour, minute, second and/or nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let from_hmsn_opt = NaiveTime::from_hms_nano_opt;
    ///
    /// assert!(from_hmsn_opt(0, 0, 0, 0).is_some());
    /// assert!(from_hmsn_opt(23, 59, 59, 999_999_999).is_some());
    /// assert!(from_hmsn_opt(23, 59, 59, 1_999_999_999).is_some()); // a leap second after 23:59:59
    /// assert!(from_hmsn_opt(24, 0, 0, 0).is_none());
    /// assert!(from_hmsn_opt(23, 60, 0, 0).is_none());
    /// assert!(from_hmsn_opt(23, 59, 60, 0).is_none());
    /// assert!(from_hmsn_opt(23, 59, 59, 2_000_000_000).is_none());
    /// ~~~~
    #[inline]
    pub fn from_hms_nano_opt(hour: u32, min: u32, sec: u32, nano: u32) -> Option<NaiveTime> {
        if hour >= 24 || min >= 60 || sec >= 60 || nano >= 2_000_000_000 {
            return None;
        }
        let secs = hour * 3600 + min * 60 + sec;
        Some(NaiveTime { secs: secs, frac: nano })
    }

    /// Makes a new `NaiveTime` from the number of seconds since midnight and nanosecond.
    ///
    /// The nanosecond part can exceed 1,000,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Panics on invalid number of seconds and/or nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let t = NaiveTime::from_num_seconds_from_midnight(86164, 12_345_678);
    /// assert_eq!(t.hour(), 23);
    /// assert_eq!(t.minute(), 56);
    /// assert_eq!(t.second(), 4);
    /// assert_eq!(t.nanosecond(), 12_345_678);
    /// ~~~~
    #[inline]
    pub fn from_num_seconds_from_midnight(secs: u32, nano: u32) -> NaiveTime {
        NaiveTime::from_num_seconds_from_midnight_opt(secs, nano).expect("invalid time")
    }

    /// Makes a new `NaiveTime` from the number of seconds since midnight and nanosecond.
    ///
    /// The nanosecond part can exceed 1,000,000,000
    /// in order to represent the [leap second](#leap-second-handling).
    ///
    /// Returns `None` on invalid number of seconds and/or nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let from_nsecs_opt = NaiveTime::from_num_seconds_from_midnight_opt;
    ///
    /// assert!(from_nsecs_opt(0, 0).is_some());
    /// assert!(from_nsecs_opt(86399, 999_999_999).is_some());
    /// assert!(from_nsecs_opt(86399, 1_999_999_999).is_some()); // a leap second after 23:59:59
    /// assert!(from_nsecs_opt(86_400, 0).is_none());
    /// assert!(from_nsecs_opt(86399, 2_000_000_000).is_none());
    /// ~~~~
    #[inline]
    pub fn from_num_seconds_from_midnight_opt(secs: u32, nano: u32) -> Option<NaiveTime> {
        if secs >= 86_400 || nano >= 2_000_000_000 {
            return None;
        }
        Some(NaiveTime { secs: secs, frac: nano })
    }

    /// Parses a string with the specified format string and returns a new `NaiveTime`.
    /// See the [`format::strftime` module](../format/strftime/index.html)
    /// on the supported escape sequences.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let parse_from_str = NaiveTime::parse_from_str;
    ///
    /// assert_eq!(parse_from_str("23:56:04", "%H:%M:%S"),
    ///            Ok(NaiveTime::from_hms(23, 56, 4)));
    /// assert_eq!(parse_from_str("pm012345.6789", "%p%I%M%S%.f"),
    ///            Ok(NaiveTime::from_hms_micro(13, 23, 45, 678_900)));
    /// ~~~~
    ///
    /// Date and offset is ignored for the purpose of parsing.
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # let parse_from_str = NaiveTime::parse_from_str;
    /// assert_eq!(parse_from_str("2014-5-17T12:34:56+09:30", "%Y-%m-%dT%H:%M:%S%z"),
    ///            Ok(NaiveTime::from_hms(12, 34, 56)));
    /// ~~~~
    ///
    /// [Leap seconds](#leap-second-handling) are correctly handled by
    /// treating any time of the form `hh:mm:60` as a leap second.
    /// (This equally applies to the formatting, so the round trip is possible.)
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # let parse_from_str = NaiveTime::parse_from_str;
    /// assert_eq!(parse_from_str("08:59:60.123", "%H:%M:%S%.f"),
    ///            Ok(NaiveTime::from_hms_milli(8, 59, 59, 1_123)));
    /// ~~~~
    ///
    /// Missing seconds are assumed to be zero,
    /// but out-of-bound times or insufficient fields are errors otherwise.
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # let parse_from_str = NaiveTime::parse_from_str;
    /// assert_eq!(parse_from_str("7:15", "%H:%M"),
    ///            Ok(NaiveTime::from_hms(7, 15, 0)));
    ///
    /// assert!(parse_from_str("04m33s", "%Mm%Ss").is_err());
    /// assert!(parse_from_str("12", "%H").is_err());
    /// assert!(parse_from_str("17:60", "%H:%M").is_err());
    /// assert!(parse_from_str("24:00:00", "%H:%M:%S").is_err());
    /// ~~~~
    ///
    /// All parsed fields should be consistent to each other, otherwise it's an error.
    /// Here `%H` is for 24-hour clocks, unlike `%I`,
    /// and thus can be independently determined without AM/PM.
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # let parse_from_str = NaiveTime::parse_from_str;
    /// assert!(parse_from_str("13:07 AM", "%H:%M %p").is_err());
    /// ~~~~
    pub fn parse_from_str(s: &str, fmt: &str) -> ParseResult<NaiveTime> {
        let mut parsed = Parsed::new();
        parse(&mut parsed, s, StrftimeItems::new(fmt))?;
        parsed.to_naive_time()
    }

    /// Adds given `Duration` to the current time,
    /// and also returns the number of *seconds*
    /// in the integral number of days ignored from the addition.
    /// (We cannot return `Duration` because it is subject to overflow or underflow.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveTime};
    ///
    /// let from_hms = NaiveTime::from_hms;
    ///
    /// assert_eq!(from_hms(3, 4, 5).overflowing_add_signed(Duration::hours(11)),
    ///            (from_hms(14, 4, 5), 0));
    /// assert_eq!(from_hms(3, 4, 5).overflowing_add_signed(Duration::hours(23)),
    ///            (from_hms(2, 4, 5), 86_400));
    /// assert_eq!(from_hms(3, 4, 5).overflowing_add_signed(Duration::hours(-7)),
    ///            (from_hms(20, 4, 5), -86_400));
    /// # }
    /// ~~~~
    #[cfg_attr(feature = "cargo-clippy", allow(cyclomatic_complexity))]
    pub fn overflowing_add_signed(&self, mut rhs: OldDuration) -> (NaiveTime, i64) {
        let mut secs = self.secs;
        let mut frac = self.frac;

        // check if `self` is a leap second and adding `rhs` would escape that leap second.
        // if it's the case, update `self` and `rhs` to involve no leap second;
        // otherwise the addition immediately finishes.
        if frac >= 1_000_000_000 {
            let rfrac = 2_000_000_000 - frac;
            if rhs >= OldDuration::nanoseconds(i64::from(rfrac)) {
                rhs = rhs - OldDuration::nanoseconds(i64::from(rfrac));
                secs += 1;
                frac = 0;
            } else if rhs < OldDuration::nanoseconds(-i64::from(frac)) {
                rhs = rhs + OldDuration::nanoseconds(i64::from(frac));
                frac = 0;
            } else {
                frac = (i64::from(frac) + rhs.num_nanoseconds().unwrap()) as u32;
                debug_assert!(frac < 2_000_000_000);
                return (NaiveTime { secs: secs, frac: frac }, 0);
            }
        }
        debug_assert!(secs <= 86_400);
        debug_assert!(frac < 1_000_000_000);

        let rhssecs = rhs.num_seconds();
        let rhsfrac = (rhs - OldDuration::seconds(rhssecs)).num_nanoseconds().unwrap();
        debug_assert_eq!(OldDuration::seconds(rhssecs) + OldDuration::nanoseconds(rhsfrac), rhs);
        let rhssecsinday = rhssecs % 86_400;
        let mut morerhssecs = rhssecs - rhssecsinday;
        let rhssecs = rhssecsinday as i32;
        let rhsfrac = rhsfrac as i32;
        debug_assert!(-86_400 < rhssecs && rhssecs < 86_400);
        debug_assert_eq!(morerhssecs % 86_400, 0);
        debug_assert!(-1_000_000_000 < rhsfrac && rhsfrac < 1_000_000_000);

        let mut secs = secs as i32 + rhssecs;
        let mut frac = frac as i32 + rhsfrac;
        debug_assert!(-86_400 < secs && secs < 2 * 86_400);
        debug_assert!(-1_000_000_000 < frac && frac < 2_000_000_000);

        if frac < 0 {
            frac += 1_000_000_000;
            secs -= 1;
        } else if frac >= 1_000_000_000 {
            frac -= 1_000_000_000;
            secs += 1;
        }
        debug_assert!(-86_400 <= secs && secs < 2 * 86_400);
        debug_assert!(0 <= frac && frac < 1_000_000_000);

        if secs < 0 {
            secs += 86_400;
            morerhssecs -= 86_400;
        } else if secs >= 86_400 {
            secs -= 86_400;
            morerhssecs += 86_400;
        }
        debug_assert!(0 <= secs && secs < 86_400);

        (NaiveTime { secs: secs as u32, frac: frac as u32 }, morerhssecs)
    }

    /// Subtracts given `Duration` from the current time,
    /// and also returns the number of *seconds*
    /// in the integral number of days ignored from the subtraction.
    /// (We cannot return `Duration` because it is subject to overflow or underflow.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveTime};
    ///
    /// let from_hms = NaiveTime::from_hms;
    ///
    /// assert_eq!(from_hms(3, 4, 5).overflowing_sub_signed(Duration::hours(2)),
    ///            (from_hms(1, 4, 5), 0));
    /// assert_eq!(from_hms(3, 4, 5).overflowing_sub_signed(Duration::hours(17)),
    ///            (from_hms(10, 4, 5), 86_400));
    /// assert_eq!(from_hms(3, 4, 5).overflowing_sub_signed(Duration::hours(-22)),
    ///            (from_hms(1, 4, 5), -86_400));
    /// # }
    /// ~~~~
    #[inline]
    pub fn overflowing_sub_signed(&self, rhs: OldDuration) -> (NaiveTime, i64) {
        let (time, rhs) = self.overflowing_add_signed(-rhs);
        (time, -rhs) // safe to negate, rhs is within +/- (2^63 / 1000)
    }

    /// Subtracts another `NaiveTime` from the current time.
    /// Returns a `Duration` within +/- 1 day.
    /// This does not overflow or underflow at all.
    ///
    /// As a part of Chrono's [leap second handling](#leap-second-handling),
    /// the subtraction assumes that **there is no leap second ever**,
    /// except when any of the `NaiveTime`s themselves represents a leap second
    /// in which case the assumption becomes that
    /// **there are exactly one (or two) leap second(s) ever**.
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveTime};
    ///
    /// let from_hmsm = NaiveTime::from_hms_milli;
    /// let since = NaiveTime::signed_duration_since;
    ///
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(3, 5, 7, 900)),
    ///            Duration::zero());
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(3, 5, 7, 875)),
    ///            Duration::milliseconds(25));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(3, 5, 6, 925)),
    ///            Duration::milliseconds(975));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(3, 5, 0, 900)),
    ///            Duration::seconds(7));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(3, 0, 7, 900)),
    ///            Duration::seconds(5 * 60));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(0, 5, 7, 900)),
    ///            Duration::seconds(3 * 3600));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(4, 5, 7, 900)),
    ///            Duration::seconds(-3600));
    /// assert_eq!(since(from_hmsm(3, 5, 7, 900), from_hmsm(2, 4, 6, 800)),
    ///            Duration::seconds(3600 + 60 + 1) + Duration::milliseconds(100));
    /// # }
    /// ~~~~
    ///
    /// Leap seconds are handled, but the subtraction assumes that
    /// there were no other leap seconds happened.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveTime};
    /// # let from_hmsm = NaiveTime::from_hms_milli;
    /// # let since = NaiveTime::signed_duration_since;
    /// assert_eq!(since(from_hmsm(3, 0, 59, 1_000), from_hmsm(3, 0, 59, 0)),
    ///            Duration::seconds(1));
    /// assert_eq!(since(from_hmsm(3, 0, 59, 1_500), from_hmsm(3, 0, 59, 0)),
    ///            Duration::milliseconds(1500));
    /// assert_eq!(since(from_hmsm(3, 0, 59, 1_000), from_hmsm(3, 0, 0, 0)),
    ///            Duration::seconds(60));
    /// assert_eq!(since(from_hmsm(3, 0, 0, 0), from_hmsm(2, 59, 59, 1_000)),
    ///            Duration::seconds(1));
    /// assert_eq!(since(from_hmsm(3, 0, 59, 1_000), from_hmsm(2, 59, 59, 1_000)),
    ///            Duration::seconds(61));
    /// # }
    /// ~~~~
    pub fn signed_duration_since(self, rhs: NaiveTime) -> OldDuration {
        //     |    |    :leap|    |    |    |    |    |    |    :leap|    |
        //     |    |    :    |    |    |    |    |    |    |    :    |    |
        // ----+----+-----*---+----+----+----+----+----+----+-------*-+----+----
        //          |   `rhs` |                             |    `self`
        //          |======================================>|       |
        //          |     |  `self.secs - rhs.secs`         |`self.frac`
        //          |====>|   |                             |======>|
        //      `rhs.frac`|========================================>|
        //          |     |   |        `self - rhs`         |       |

        use core::cmp::Ordering;

        let secs = i64::from(self.secs) - i64::from(rhs.secs);
        let frac = i64::from(self.frac) - i64::from(rhs.frac);

        // `secs` may contain a leap second yet to be counted
        let adjust = match self.secs.cmp(&rhs.secs) {
            Ordering::Greater => {
                if rhs.frac >= 1_000_000_000 {
                    1
                } else {
                    0
                }
            }
            Ordering::Equal => 0,
            Ordering::Less => {
                if self.frac >= 1_000_000_000 {
                    -1
                } else {
                    0
                }
            }
        };

        OldDuration::seconds(secs + adjust) + OldDuration::nanoseconds(frac)
    }

    /// Formats the time with the specified formatting items.
    /// Otherwise it is the same as the ordinary [`format`](#method.format) method.
    ///
    /// The `Iterator` of items should be `Clone`able,
    /// since the resulting `DelayedFormat` value may be formatted multiple times.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    /// use chrono::format::strftime::StrftimeItems;
    ///
    /// let fmt = StrftimeItems::new("%H:%M:%S");
    /// let t = NaiveTime::from_hms(23, 56, 4);
    /// assert_eq!(t.format_with_items(fmt.clone()).to_string(), "23:56:04");
    /// assert_eq!(t.format("%H:%M:%S").to_string(),             "23:56:04");
    /// ~~~~
    ///
    /// The resulting `DelayedFormat` can be formatted directly via the `Display` trait.
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # use chrono::format::strftime::StrftimeItems;
    /// # let fmt = StrftimeItems::new("%H:%M:%S").clone();
    /// # let t = NaiveTime::from_hms(23, 56, 4);
    /// assert_eq!(format!("{}", t.format_with_items(fmt)), "23:56:04");
    /// ~~~~
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format_with_items<'a, I, B>(&self, items: I) -> DelayedFormat<I>
    where
        I: Iterator<Item = B> + Clone,
        B: Borrow<Item<'a>>,
    {
        DelayedFormat::new(None, Some(*self), items)
    }

    /// Formats the time with the specified format string.
    /// See the [`format::strftime` module](../format/strftime/index.html)
    /// on the supported escape sequences.
    ///
    /// This returns a `DelayedFormat`,
    /// which gets converted to a string only when actual formatting happens.
    /// You may use the `to_string` method to get a `String`,
    /// or just feed it into `print!` and other formatting macros.
    /// (In this way it avoids the redundant memory allocation.)
    ///
    /// A wrong format string does *not* issue an error immediately.
    /// Rather, converting or formatting the `DelayedFormat` fails.
    /// You are recommended to immediately use `DelayedFormat` for this reason.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveTime;
    ///
    /// let t = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(t.format("%H:%M:%S").to_string(), "23:56:04");
    /// assert_eq!(t.format("%H:%M:%S%.6f").to_string(), "23:56:04.012345");
    /// assert_eq!(t.format("%-I:%M %p").to_string(), "11:56 PM");
    /// ~~~~
    ///
    /// The resulting `DelayedFormat` can be formatted directly via the `Display` trait.
    ///
    /// ~~~~
    /// # use chrono::NaiveTime;
    /// # let t = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(format!("{}", t.format("%H:%M:%S")), "23:56:04");
    /// assert_eq!(format!("{}", t.format("%H:%M:%S%.6f")), "23:56:04.012345");
    /// assert_eq!(format!("{}", t.format("%-I:%M %p")), "11:56 PM");
    /// ~~~~
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.format_with_items(StrftimeItems::new(fmt))
    }

    /// Returns a triple of the hour, minute and second numbers.
    fn hms(&self) -> (u32, u32, u32) {
        let (mins, sec) = div_mod_floor(self.secs, 60);
        let (hour, min) = div_mod_floor(mins, 60);
        (hour, min, sec)
    }
}

impl Timelike for NaiveTime {
    /// Returns the hour number from 0 to 23.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// assert_eq!(NaiveTime::from_hms(0, 0, 0).hour(), 0);
    /// assert_eq!(NaiveTime::from_hms_nano(23, 56, 4, 12_345_678).hour(), 23);
    /// ~~~~
    #[inline]
    fn hour(&self) -> u32 {
        self.hms().0
    }

    /// Returns the minute number from 0 to 59.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// assert_eq!(NaiveTime::from_hms(0, 0, 0).minute(), 0);
    /// assert_eq!(NaiveTime::from_hms_nano(23, 56, 4, 12_345_678).minute(), 56);
    /// ~~~~
    #[inline]
    fn minute(&self) -> u32 {
        self.hms().1
    }

    /// Returns the second number from 0 to 59.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// assert_eq!(NaiveTime::from_hms(0, 0, 0).second(), 0);
    /// assert_eq!(NaiveTime::from_hms_nano(23, 56, 4, 12_345_678).second(), 4);
    /// ~~~~
    ///
    /// This method never returns 60 even when it is a leap second.
    /// ([Why?](#leap-second-handling))
    /// Use the proper [formatting method](#method.format) to get a human-readable representation.
    ///
    /// ~~~~
    /// # use chrono::{NaiveTime, Timelike};
    /// let leap = NaiveTime::from_hms_milli(23, 59, 59, 1_000);
    /// assert_eq!(leap.second(), 59);
    /// assert_eq!(leap.format("%H:%M:%S").to_string(), "23:59:60");
    /// ~~~~
    #[inline]
    fn second(&self) -> u32 {
        self.hms().2
    }

    /// Returns the number of nanoseconds since the whole non-leap second.
    /// The range from 1,000,000,000 to 1,999,999,999 represents
    /// the [leap second](#leap-second-handling).
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// assert_eq!(NaiveTime::from_hms(0, 0, 0).nanosecond(), 0);
    /// assert_eq!(NaiveTime::from_hms_nano(23, 56, 4, 12_345_678).nanosecond(), 12_345_678);
    /// ~~~~
    ///
    /// Leap seconds may have seemingly out-of-range return values.
    /// You can reduce the range with `time.nanosecond() % 1_000_000_000`, or
    /// use the proper [formatting method](#method.format) to get a human-readable representation.
    ///
    /// ~~~~
    /// # use chrono::{NaiveTime, Timelike};
    /// let leap = NaiveTime::from_hms_milli(23, 59, 59, 1_000);
    /// assert_eq!(leap.nanosecond(), 1_000_000_000);
    /// assert_eq!(leap.format("%H:%M:%S%.9f").to_string(), "23:59:60.000000000");
    /// ~~~~
    #[inline]
    fn nanosecond(&self) -> u32 {
        self.frac
    }

    /// Makes a new `NaiveTime` with the hour number changed.
    ///
    /// Returns `None` when the resulting `NaiveTime` would be invalid.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let dt = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(dt.with_hour(7), Some(NaiveTime::from_hms_nano(7, 56, 4, 12_345_678)));
    /// assert_eq!(dt.with_hour(24), None);
    /// ~~~~
    #[inline]
    fn with_hour(&self, hour: u32) -> Option<NaiveTime> {
        if hour >= 24 {
            return None;
        }
        let secs = hour * 3600 + self.secs % 3600;
        Some(NaiveTime { secs: secs, ..*self })
    }

    /// Makes a new `NaiveTime` with the minute number changed.
    ///
    /// Returns `None` when the resulting `NaiveTime` would be invalid.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let dt = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(dt.with_minute(45), Some(NaiveTime::from_hms_nano(23, 45, 4, 12_345_678)));
    /// assert_eq!(dt.with_minute(60), None);
    /// ~~~~
    #[inline]
    fn with_minute(&self, min: u32) -> Option<NaiveTime> {
        if min >= 60 {
            return None;
        }
        let secs = self.secs / 3600 * 3600 + min * 60 + self.secs % 60;
        Some(NaiveTime { secs: secs, ..*self })
    }

    /// Makes a new `NaiveTime` with the second number changed.
    ///
    /// Returns `None` when the resulting `NaiveTime` would be invalid.
    /// As with the [`second`](#method.second) method,
    /// the input range is restricted to 0 through 59.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let dt = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(dt.with_second(17), Some(NaiveTime::from_hms_nano(23, 56, 17, 12_345_678)));
    /// assert_eq!(dt.with_second(60), None);
    /// ~~~~
    #[inline]
    fn with_second(&self, sec: u32) -> Option<NaiveTime> {
        if sec >= 60 {
            return None;
        }
        let secs = self.secs / 60 * 60 + sec;
        Some(NaiveTime { secs: secs, ..*self })
    }

    /// Makes a new `NaiveTime` with nanoseconds since the whole non-leap second changed.
    ///
    /// Returns `None` when the resulting `NaiveTime` would be invalid.
    /// As with the [`nanosecond`](#method.nanosecond) method,
    /// the input range can exceed 1,000,000,000 for leap seconds.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// let dt = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(dt.with_nanosecond(333_333_333),
    ///            Some(NaiveTime::from_hms_nano(23, 56, 4, 333_333_333)));
    /// assert_eq!(dt.with_nanosecond(2_000_000_000), None);
    /// ~~~~
    ///
    /// Leap seconds can theoretically follow *any* whole second.
    /// The following would be a proper leap second at the time zone offset of UTC-00:03:57
    /// (there are several historical examples comparable to this "non-sense" offset),
    /// and therefore is allowed.
    ///
    /// ~~~~
    /// # use chrono::{NaiveTime, Timelike};
    /// # let dt = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
    /// assert_eq!(dt.with_nanosecond(1_333_333_333),
    ///            Some(NaiveTime::from_hms_nano(23, 56, 4, 1_333_333_333)));
    /// ~~~~
    #[inline]
    fn with_nanosecond(&self, nano: u32) -> Option<NaiveTime> {
        if nano >= 2_000_000_000 {
            return None;
        }
        Some(NaiveTime { frac: nano, ..*self })
    }

    /// Returns the number of non-leap seconds past the last midnight.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveTime, Timelike};
    ///
    /// assert_eq!(NaiveTime::from_hms(1, 2, 3).num_seconds_from_midnight(),
    ///            3723);
    /// assert_eq!(NaiveTime::from_hms_nano(23, 56, 4, 12_345_678).num_seconds_from_midnight(),
    ///            86164);
    /// assert_eq!(NaiveTime::from_hms_milli(23, 59, 59, 1_000).num_seconds_from_midnight(),
    ///            86399);
    /// ~~~~
    #[inline]
    fn num_seconds_from_midnight(&self) -> u32 {
        self.secs // do not repeat the calculation!
    }
}

/// `NaiveTime` can be used as a key to the hash maps (in principle).
///
/// Practically this also takes account of fractional seconds, so it is not recommended.
/// (For the obvious reason this also distinguishes leap seconds from non-leap seconds.)
impl hash::Hash for NaiveTime {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.secs.hash(state);
        self.frac.hash(state);
    }
}

/// An addition of `Duration` to `NaiveTime` wraps around and never overflows or underflows.
/// In particular the addition ignores integral number of days.
///
/// As a part of Chrono's [leap second handling](#leap-second-handling),
/// the addition assumes that **there is no leap second ever**,
/// except when the `NaiveTime` itself represents a leap second
/// in which case the assumption becomes that **there is exactly a single leap second ever**.
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveTime};
///
/// let from_hmsm = NaiveTime::from_hms_milli;
///
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::zero(),                  from_hmsm(3, 5, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(1),              from_hmsm(3, 5, 8, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(-1),             from_hmsm(3, 5, 6, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(60 + 4),         from_hmsm(3, 6, 11, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(7*60*60 - 6*60), from_hmsm(9, 59, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::milliseconds(80),        from_hmsm(3, 5, 7, 80));
/// assert_eq!(from_hmsm(3, 5, 7, 950) + Duration::milliseconds(280),     from_hmsm(3, 5, 8, 230));
/// assert_eq!(from_hmsm(3, 5, 7, 950) + Duration::milliseconds(-980),    from_hmsm(3, 5, 6, 970));
/// # }
/// ~~~~
///
/// The addition wraps around.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveTime};
/// # let from_hmsm = NaiveTime::from_hms_milli;
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(22*60*60), from_hmsm(1, 5, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::seconds(-8*60*60), from_hmsm(19, 5, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) + Duration::days(800),         from_hmsm(3, 5, 7, 0));
/// # }
/// ~~~~
///
/// Leap seconds are handled, but the addition assumes that it is the only leap second happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveTime};
/// # let from_hmsm = NaiveTime::from_hms_milli;
/// let leap = from_hmsm(3, 5, 59, 1_300);
/// assert_eq!(leap + Duration::zero(),             from_hmsm(3, 5, 59, 1_300));
/// assert_eq!(leap + Duration::milliseconds(-500), from_hmsm(3, 5, 59, 800));
/// assert_eq!(leap + Duration::milliseconds(500),  from_hmsm(3, 5, 59, 1_800));
/// assert_eq!(leap + Duration::milliseconds(800),  from_hmsm(3, 6, 0, 100));
/// assert_eq!(leap + Duration::seconds(10),        from_hmsm(3, 6, 9, 300));
/// assert_eq!(leap + Duration::seconds(-10),       from_hmsm(3, 5, 50, 300));
/// assert_eq!(leap + Duration::days(1),            from_hmsm(3, 5, 59, 300));
/// # }
/// ~~~~
impl Add<OldDuration> for NaiveTime {
    type Output = NaiveTime;

    #[inline]
    fn add(self, rhs: OldDuration) -> NaiveTime {
        self.overflowing_add_signed(rhs).0
    }
}

impl AddAssign<OldDuration> for NaiveTime {
    #[inline]
    fn add_assign(&mut self, rhs: OldDuration) {
        *self = self.add(rhs);
    }
}

/// A subtraction of `Duration` from `NaiveTime` wraps around and never overflows or underflows.
/// In particular the addition ignores integral number of days.
/// It is the same as the addition with a negated `Duration`.
///
/// As a part of Chrono's [leap second handling](#leap-second-handling),
/// the addition assumes that **there is no leap second ever**,
/// except when the `NaiveTime` itself represents a leap second
/// in which case the assumption becomes that **there is exactly a single leap second ever**.
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveTime};
///
/// let from_hmsm = NaiveTime::from_hms_milli;
///
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::zero(),                  from_hmsm(3, 5, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::seconds(1),              from_hmsm(3, 5, 6, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::seconds(60 + 5),         from_hmsm(3, 4, 2, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::seconds(2*60*60 + 6*60), from_hmsm(0, 59, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::milliseconds(80),        from_hmsm(3, 5, 6, 920));
/// assert_eq!(from_hmsm(3, 5, 7, 950) - Duration::milliseconds(280),     from_hmsm(3, 5, 7, 670));
/// # }
/// ~~~~
///
/// The subtraction wraps around.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveTime};
/// # let from_hmsm = NaiveTime::from_hms_milli;
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::seconds(8*60*60), from_hmsm(19, 5, 7, 0));
/// assert_eq!(from_hmsm(3, 5, 7, 0) - Duration::days(800),        from_hmsm(3, 5, 7, 0));
/// # }
/// ~~~~
///
/// Leap seconds are handled, but the subtraction assumes that it is the only leap second happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveTime};
/// # let from_hmsm = NaiveTime::from_hms_milli;
/// let leap = from_hmsm(3, 5, 59, 1_300);
/// assert_eq!(leap - Duration::zero(),            from_hmsm(3, 5, 59, 1_300));
/// assert_eq!(leap - Duration::milliseconds(200), from_hmsm(3, 5, 59, 1_100));
/// assert_eq!(leap - Duration::milliseconds(500), from_hmsm(3, 5, 59, 800));
/// assert_eq!(leap - Duration::seconds(60),       from_hmsm(3, 5, 0, 300));
/// assert_eq!(leap - Duration::days(1),           from_hmsm(3, 6, 0, 300));
/// # }
/// ~~~~
impl Sub<OldDuration> for NaiveTime {
    type Output = NaiveTime;

    #[inline]
    fn sub(self, rhs: OldDuration) -> NaiveTime {
        self.overflowing_sub_signed(rhs).0
    }
}

impl SubAssign<OldDuration> for NaiveTime {
    #[inline]
    fn sub_assign(&mut self, rhs: OldDuration) {
        *self = self.sub(rhs);
    }
}

/// Subtracts another `NaiveTime` from the current time.
/// Returns a `Duration` within +/- 1 day.
/// This does not overflow or underflow at all.
///
/// As a part of Chrono's [leap second handling](#leap-second-handling),
/// the subtraction assumes that **there is no leap second ever**,
/// except when any of the `NaiveTime`s themselves represents a leap second
/// in which case the assumption becomes that
/// **there are exactly one (or two) leap second(s) ever**.
///
/// The implementation is a wrapper around
/// [`NaiveTime::signed_duration_since`](#method.signed_duration_since).
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveTime};
///
/// let from_hmsm = NaiveTime::from_hms_milli;
///
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(3, 5, 7, 900), Duration::zero());
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(3, 5, 7, 875), Duration::milliseconds(25));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(3, 5, 6, 925), Duration::milliseconds(975));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(3, 5, 0, 900), Duration::seconds(7));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(3, 0, 7, 900), Duration::seconds(5 * 60));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(0, 5, 7, 900), Duration::seconds(3 * 3600));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(4, 5, 7, 900), Duration::seconds(-3600));
/// assert_eq!(from_hmsm(3, 5, 7, 900) - from_hmsm(2, 4, 6, 800),
///            Duration::seconds(3600 + 60 + 1) + Duration::milliseconds(100));
/// # }
/// ~~~~
///
/// Leap seconds are handled, but the subtraction assumes that
/// there were no other leap seconds happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveTime};
/// # let from_hmsm = NaiveTime::from_hms_milli;
/// assert_eq!(from_hmsm(3, 0, 59, 1_000) - from_hmsm(3, 0, 59, 0), Duration::seconds(1));
/// assert_eq!(from_hmsm(3, 0, 59, 1_500) - from_hmsm(3, 0, 59, 0),
///            Duration::milliseconds(1500));
/// assert_eq!(from_hmsm(3, 0, 59, 1_000) - from_hmsm(3, 0, 0, 0), Duration::seconds(60));
/// assert_eq!(from_hmsm(3, 0, 0, 0) - from_hmsm(2, 59, 59, 1_000), Duration::seconds(1));
/// assert_eq!(from_hmsm(3, 0, 59, 1_000) - from_hmsm(2, 59, 59, 1_000),
///            Duration::seconds(61));
/// # }
/// ~~~~
impl Sub<NaiveTime> for NaiveTime {
    type Output = OldDuration;

    #[inline]
    fn sub(self, rhs: NaiveTime) -> OldDuration {
        self.signed_duration_since(rhs)
    }
}

/// The `Debug` output of the naive time `t` is the same as
/// [`t.format("%H:%M:%S%.f")`](../format/strftime/index.html).
///
/// The string printed can be readily parsed via the `parse` method on `str`.
///
/// It should be noted that, for leap seconds not on the minute boundary,
/// it may print a representation not distinguishable from non-leap seconds.
/// This doesn't matter in practice, since such leap seconds never happened.
/// (By the time of the first leap second on 1972-06-30,
/// every time zone offset around the world has standardized to the 5-minute alignment.)
///
/// # Example
///
/// ~~~~
/// use chrono::NaiveTime;
///
/// assert_eq!(format!("{:?}", NaiveTime::from_hms(23, 56, 4)),              "23:56:04");
/// assert_eq!(format!("{:?}", NaiveTime::from_hms_milli(23, 56, 4, 12)),    "23:56:04.012");
/// assert_eq!(format!("{:?}", NaiveTime::from_hms_micro(23, 56, 4, 1234)),  "23:56:04.001234");
/// assert_eq!(format!("{:?}", NaiveTime::from_hms_nano(23, 56, 4, 123456)), "23:56:04.000123456");
/// ~~~~
///
/// Leap seconds may also be used.
///
/// ~~~~
/// # use chrono::NaiveTime;
/// assert_eq!(format!("{:?}", NaiveTime::from_hms_milli(6, 59, 59, 1_500)), "06:59:60.500");
/// ~~~~
impl fmt::Debug for NaiveTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (hour, min, sec) = self.hms();
        let (sec, nano) = if self.frac >= 1_000_000_000 {
            (sec + 1, self.frac - 1_000_000_000)
        } else {
            (sec, self.frac)
        };

        write!(f, "{:02}:{:02}:{:02}", hour, min, sec)?;
        if nano == 0 {
            Ok(())
        } else if nano % 1_000_000 == 0 {
            write!(f, ".{:03}", nano / 1_000_000)
        } else if nano % 1_000 == 0 {
            write!(f, ".{:06}", nano / 1_000)
        } else {
            write!(f, ".{:09}", nano)
        }
    }
}

/// The `Display` output of the naive time `t` is the same as
/// [`t.format("%H:%M:%S%.f")`](../format/strftime/index.html).
///
/// The string printed can be readily parsed via the `parse` method on `str`.
///
/// It should be noted that, for leap seconds not on the minute boundary,
/// it may print a representation not distinguishable from non-leap seconds.
/// This doesn't matter in practice, since such leap seconds never happened.
/// (By the time of the first leap second on 1972-06-30,
/// every time zone offset around the world has standardized to the 5-minute alignment.)
///
/// # Example
///
/// ~~~~
/// use chrono::NaiveTime;
///
/// assert_eq!(format!("{}", NaiveTime::from_hms(23, 56, 4)),              "23:56:04");
/// assert_eq!(format!("{}", NaiveTime::from_hms_milli(23, 56, 4, 12)),    "23:56:04.012");
/// assert_eq!(format!("{}", NaiveTime::from_hms_micro(23, 56, 4, 1234)),  "23:56:04.001234");
/// assert_eq!(format!("{}", NaiveTime::from_hms_nano(23, 56, 4, 123456)), "23:56:04.000123456");
/// ~~~~
///
/// Leap seconds may also be used.
///
/// ~~~~
/// # use chrono::NaiveTime;
/// assert_eq!(format!("{}", NaiveTime::from_hms_milli(6, 59, 59, 1_500)), "06:59:60.500");
/// ~~~~
impl fmt::Display for NaiveTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Parsing a `str` into a `NaiveTime` uses the same format,
/// [`%H:%M:%S%.f`](../format/strftime/index.html), as in `Debug` and `Display`.
///
/// # Example
///
/// ~~~~
/// use chrono::NaiveTime;
///
/// let t = NaiveTime::from_hms(23, 56, 4);
/// assert_eq!("23:56:04".parse::<NaiveTime>(), Ok(t));
///
/// let t = NaiveTime::from_hms_nano(23, 56, 4, 12_345_678);
/// assert_eq!("23:56:4.012345678".parse::<NaiveTime>(), Ok(t));
///
/// let t = NaiveTime::from_hms_nano(23, 59, 59, 1_234_567_890); // leap second
/// assert_eq!("23:59:60.23456789".parse::<NaiveTime>(), Ok(t));
///
/// assert!("foo".parse::<NaiveTime>().is_err());
/// ~~~~
impl str::FromStr for NaiveTime {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<NaiveTime> {
        const ITEMS: &'static [Item<'static>] = &[
            Item::Numeric(Numeric::Hour, Pad::Zero),
            Item::Space(""),
            Item::Literal(":"),
            Item::Numeric(Numeric::Minute, Pad::Zero),
            Item::Space(""),
            Item::Literal(":"),
            Item::Numeric(Numeric::Second, Pad::Zero),
            Item::Fixed(Fixed::Nanosecond),
            Item::Space(""),
        ];

        let mut parsed = Parsed::new();
        parse(&mut parsed, s, ITEMS.iter())?;
        parsed.to_naive_time()
    }
}

#[cfg(all(test, any(feature = "rustc-serialize", feature = "serde")))]
fn test_encodable_json<F, E>(to_string: F)
where
    F: Fn(&NaiveTime) -> Result<String, E>,
    E: ::std::fmt::Debug,
{
    assert_eq!(to_string(&NaiveTime::from_hms(0, 0, 0)).ok(), Some(r#""00:00:00""#.into()));
    assert_eq!(
        to_string(&NaiveTime::from_hms_milli(0, 0, 0, 950)).ok(),
        Some(r#""00:00:00.950""#.into())
    );
    assert_eq!(
        to_string(&NaiveTime::from_hms_milli(0, 0, 59, 1_000)).ok(),
        Some(r#""00:00:60""#.into())
    );
    assert_eq!(to_string(&NaiveTime::from_hms(0, 1, 2)).ok(), Some(r#""00:01:02""#.into()));
    assert_eq!(
        to_string(&NaiveTime::from_hms_nano(3, 5, 7, 98765432)).ok(),
        Some(r#""03:05:07.098765432""#.into())
    );
    assert_eq!(to_string(&NaiveTime::from_hms(7, 8, 9)).ok(), Some(r#""07:08:09""#.into()));
    assert_eq!(
        to_string(&NaiveTime::from_hms_micro(12, 34, 56, 789)).ok(),
        Some(r#""12:34:56.000789""#.into())
    );
    assert_eq!(
        to_string(&NaiveTime::from_hms_nano(23, 59, 59, 1_999_999_999)).ok(),
        Some(r#""23:59:60.999999999""#.into())
    );
}

#[cfg(all(test, any(feature = "rustc-serialize", feature = "serde")))]
fn test_decodable_json<F, E>(from_str: F)
where
    F: Fn(&str) -> Result<NaiveTime, E>,
    E: ::std::fmt::Debug,
{
    assert_eq!(from_str(r#""00:00:00""#).ok(), Some(NaiveTime::from_hms(0, 0, 0)));
    assert_eq!(from_str(r#""0:0:0""#).ok(), Some(NaiveTime::from_hms(0, 0, 0)));
    assert_eq!(from_str(r#""00:00:00.950""#).ok(), Some(NaiveTime::from_hms_milli(0, 0, 0, 950)));
    assert_eq!(from_str(r#""0:0:0.95""#).ok(), Some(NaiveTime::from_hms_milli(0, 0, 0, 950)));
    assert_eq!(from_str(r#""00:00:60""#).ok(), Some(NaiveTime::from_hms_milli(0, 0, 59, 1_000)));
    assert_eq!(from_str(r#""00:01:02""#).ok(), Some(NaiveTime::from_hms(0, 1, 2)));
    assert_eq!(
        from_str(r#""03:05:07.098765432""#).ok(),
        Some(NaiveTime::from_hms_nano(3, 5, 7, 98765432))
    );
    assert_eq!(from_str(r#""07:08:09""#).ok(), Some(NaiveTime::from_hms(7, 8, 9)));
    assert_eq!(
        from_str(r#""12:34:56.000789""#).ok(),
        Some(NaiveTime::from_hms_micro(12, 34, 56, 789))
    );
    assert_eq!(
        from_str(r#""23:59:60.999999999""#).ok(),
        Some(NaiveTime::from_hms_nano(23, 59, 59, 1_999_999_999))
    );
    assert_eq!(
        from_str(r#""23:59:60.9999999999997""#).ok(), // excess digits are ignored
        Some(NaiveTime::from_hms_nano(23, 59, 59, 1_999_999_999))
    );

    // bad formats
    assert!(from_str(r#""""#).is_err());
    assert!(from_str(r#""000000""#).is_err());
    assert!(from_str(r#""00:00:61""#).is_err());
    assert!(from_str(r#""00:60:00""#).is_err());
    assert!(from_str(r#""24:00:00""#).is_err());
    assert!(from_str(r#""23:59:59,1""#).is_err());
    assert!(from_str(r#""012:34:56""#).is_err());
    assert!(from_str(r#""hh:mm:ss""#).is_err());
    assert!(from_str(r#"0"#).is_err());
    assert!(from_str(r#"86399"#).is_err());
    assert!(from_str(r#"{}"#).is_err());
    // pre-0.3.0 rustc-serialize format is now invalid
    assert!(from_str(r#"{"secs":0,"frac":0}"#).is_err());
    assert!(from_str(r#"null"#).is_err());
}

#[cfg(feature = "rustc-serialize")]
mod rustc_serialize {
    use super::NaiveTime;
    use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};

    impl Encodable for NaiveTime {
        fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
            format!("{:?}", self).encode(s)
        }
    }

    impl Decodable for NaiveTime {
        fn decode<D: Decoder>(d: &mut D) -> Result<NaiveTime, D::Error> {
            d.read_str()?.parse().map_err(|_| d.error("invalid time"))
        }
    }

    #[cfg(test)]
    use rustc_serialize::json;

    #[test]
    fn test_encodable() {
        super::test_encodable_json(json::encode);
    }

    #[test]
    fn test_decodable() {
        super::test_decodable_json(json::decode);
    }
}

#[cfg(feature = "serde")]
mod serde {
    use super::NaiveTime;
    use core::fmt;
    use serdelib::{de, ser};

    // TODO not very optimized for space (binary formats would want something better)
    // TODO round-trip for general leap seconds (not just those with second = 60)

    impl ser::Serialize for NaiveTime {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.collect_str(&self)
        }
    }

    struct NaiveTimeVisitor;

    impl<'de> de::Visitor<'de> for NaiveTimeVisitor {
        type Value = NaiveTime;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a formatted time string")
        }

        fn visit_str<E>(self, value: &str) -> Result<NaiveTime, E>
        where
            E: de::Error,
        {
            value.parse().map_err(E::custom)
        }
    }

    impl<'de> de::Deserialize<'de> for NaiveTime {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_str(NaiveTimeVisitor)
        }
    }

    #[cfg(test)]
    extern crate bincode;
    #[cfg(test)]
    extern crate serde_json;

    #[test]
    fn test_serde_serialize() {
        super::test_encodable_json(self::serde_json::to_string);
    }

    #[test]
    fn test_serde_deserialize() {
        super::test_decodable_json(|input| self::serde_json::from_str(&input));
    }

    #[test]
    fn test_serde_bincode() {
        // Bincode is relevant to test separately from JSON because
        // it is not self-describing.
        use self::bincode::{deserialize, serialize, Infinite};

        let t = NaiveTime::from_hms_nano(3, 5, 7, 98765432);
        let encoded = serialize(&t, Infinite).unwrap();
        let decoded: NaiveTime = deserialize(&encoded).unwrap();
        assert_eq!(t, decoded);
    }
}

#[cfg(test)]
mod tests {
    use super::NaiveTime;
    use oldtime::Duration;
    use std::u32;
    use Timelike;

    #[test]
    fn test_time_from_hms_milli() {
        assert_eq!(
            NaiveTime::from_hms_milli_opt(3, 5, 7, 0),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 0))
        );
        assert_eq!(
            NaiveTime::from_hms_milli_opt(3, 5, 7, 777),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 777_000_000))
        );
        assert_eq!(
            NaiveTime::from_hms_milli_opt(3, 5, 7, 1_999),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 1_999_000_000))
        );
        assert_eq!(NaiveTime::from_hms_milli_opt(3, 5, 7, 2_000), None);
        assert_eq!(NaiveTime::from_hms_milli_opt(3, 5, 7, 5_000), None); // overflow check
        assert_eq!(NaiveTime::from_hms_milli_opt(3, 5, 7, u32::MAX), None);
    }

    #[test]
    fn test_time_from_hms_micro() {
        assert_eq!(
            NaiveTime::from_hms_micro_opt(3, 5, 7, 0),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 0))
        );
        assert_eq!(
            NaiveTime::from_hms_micro_opt(3, 5, 7, 333),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 333_000))
        );
        assert_eq!(
            NaiveTime::from_hms_micro_opt(3, 5, 7, 777_777),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 777_777_000))
        );
        assert_eq!(
            NaiveTime::from_hms_micro_opt(3, 5, 7, 1_999_999),
            Some(NaiveTime::from_hms_nano(3, 5, 7, 1_999_999_000))
        );
        assert_eq!(NaiveTime::from_hms_micro_opt(3, 5, 7, 2_000_000), None);
        assert_eq!(NaiveTime::from_hms_micro_opt(3, 5, 7, 5_000_000), None); // overflow check
        assert_eq!(NaiveTime::from_hms_micro_opt(3, 5, 7, u32::MAX), None);
    }

    #[test]
    fn test_time_hms() {
        assert_eq!(NaiveTime::from_hms(3, 5, 7).hour(), 3);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_hour(0), Some(NaiveTime::from_hms(0, 5, 7)));
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_hour(23), Some(NaiveTime::from_hms(23, 5, 7)));
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_hour(24), None);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_hour(u32::MAX), None);

        assert_eq!(NaiveTime::from_hms(3, 5, 7).minute(), 5);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_minute(0), Some(NaiveTime::from_hms(3, 0, 7)));
        assert_eq!(
            NaiveTime::from_hms(3, 5, 7).with_minute(59),
            Some(NaiveTime::from_hms(3, 59, 7))
        );
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_minute(60), None);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_minute(u32::MAX), None);

        assert_eq!(NaiveTime::from_hms(3, 5, 7).second(), 7);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_second(0), Some(NaiveTime::from_hms(3, 5, 0)));
        assert_eq!(
            NaiveTime::from_hms(3, 5, 7).with_second(59),
            Some(NaiveTime::from_hms(3, 5, 59))
        );
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_second(60), None);
        assert_eq!(NaiveTime::from_hms(3, 5, 7).with_second(u32::MAX), None);
    }

    #[test]
    fn test_time_add() {
        macro_rules! check {
            ($lhs:expr, $rhs:expr, $sum:expr) => {{
                assert_eq!($lhs + $rhs, $sum);
                //assert_eq!($rhs + $lhs, $sum);
            }};
        }

        let hmsm = |h, m, s, mi| NaiveTime::from_hms_milli(h, m, s, mi);

        check!(hmsm(3, 5, 7, 900), Duration::zero(), hmsm(3, 5, 7, 900));
        check!(hmsm(3, 5, 7, 900), Duration::milliseconds(100), hmsm(3, 5, 8, 0));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(-1800), hmsm(3, 5, 6, 500));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(-800), hmsm(3, 5, 7, 500));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(-100), hmsm(3, 5, 7, 1_200));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(100), hmsm(3, 5, 7, 1_400));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(800), hmsm(3, 5, 8, 100));
        check!(hmsm(3, 5, 7, 1_300), Duration::milliseconds(1800), hmsm(3, 5, 9, 100));
        check!(hmsm(3, 5, 7, 900), Duration::seconds(86399), hmsm(3, 5, 6, 900)); // overwrap
        check!(hmsm(3, 5, 7, 900), Duration::seconds(-86399), hmsm(3, 5, 8, 900));
        check!(hmsm(3, 5, 7, 900), Duration::days(12345), hmsm(3, 5, 7, 900));
        check!(hmsm(3, 5, 7, 1_300), Duration::days(1), hmsm(3, 5, 7, 300));
        check!(hmsm(3, 5, 7, 1_300), Duration::days(-1), hmsm(3, 5, 8, 300));

        // regression tests for #37
        check!(hmsm(0, 0, 0, 0), Duration::milliseconds(-990), hmsm(23, 59, 59, 10));
        check!(hmsm(0, 0, 0, 0), Duration::milliseconds(-9990), hmsm(23, 59, 50, 10));
    }

    #[test]
    fn test_time_overflowing_add() {
        let hmsm = NaiveTime::from_hms_milli;

        assert_eq!(
            hmsm(3, 4, 5, 678).overflowing_add_signed(Duration::hours(11)),
            (hmsm(14, 4, 5, 678), 0)
        );
        assert_eq!(
            hmsm(3, 4, 5, 678).overflowing_add_signed(Duration::hours(23)),
            (hmsm(2, 4, 5, 678), 86_400)
        );
        assert_eq!(
            hmsm(3, 4, 5, 678).overflowing_add_signed(Duration::hours(-7)),
            (hmsm(20, 4, 5, 678), -86_400)
        );

        // overflowing_add_signed with leap seconds may be counter-intuitive
        assert_eq!(
            hmsm(3, 4, 5, 1_678).overflowing_add_signed(Duration::days(1)),
            (hmsm(3, 4, 5, 678), 86_400)
        );
        assert_eq!(
            hmsm(3, 4, 5, 1_678).overflowing_add_signed(Duration::days(-1)),
            (hmsm(3, 4, 6, 678), -86_400)
        );
    }

    #[test]
    fn test_time_addassignment() {
        let hms = NaiveTime::from_hms;
        let mut time = hms(12, 12, 12);
        time += Duration::hours(10);
        assert_eq!(time, hms(22, 12, 12));
        time += Duration::hours(10);
        assert_eq!(time, hms(8, 12, 12));
    }

    #[test]
    fn test_time_subassignment() {
        let hms = NaiveTime::from_hms;
        let mut time = hms(12, 12, 12);
        time -= Duration::hours(10);
        assert_eq!(time, hms(2, 12, 12));
        time -= Duration::hours(10);
        assert_eq!(time, hms(16, 12, 12));
    }

    #[test]
    fn test_time_sub() {
        macro_rules! check {
            ($lhs:expr, $rhs:expr, $diff:expr) => {{
                // `time1 - time2 = duration` is equivalent to `time2 - time1 = -duration`
                assert_eq!($lhs.signed_duration_since($rhs), $diff);
                assert_eq!($rhs.signed_duration_since($lhs), -$diff);
            }};
        }

        let hmsm = |h, m, s, mi| NaiveTime::from_hms_milli(h, m, s, mi);

        check!(hmsm(3, 5, 7, 900), hmsm(3, 5, 7, 900), Duration::zero());
        check!(hmsm(3, 5, 7, 900), hmsm(3, 5, 7, 600), Duration::milliseconds(300));
        check!(hmsm(3, 5, 7, 200), hmsm(2, 4, 6, 200), Duration::seconds(3600 + 60 + 1));
        check!(
            hmsm(3, 5, 7, 200),
            hmsm(2, 4, 6, 300),
            Duration::seconds(3600 + 60) + Duration::milliseconds(900)
        );

        // treats the leap second as if it coincides with the prior non-leap second,
        // as required by `time1 - time2 = duration` and `time2 - time1 = -duration` equivalence.
        check!(hmsm(3, 5, 7, 200), hmsm(3, 5, 6, 1_800), Duration::milliseconds(400));
        check!(hmsm(3, 5, 7, 1_200), hmsm(3, 5, 6, 1_800), Duration::milliseconds(1400));
        check!(hmsm(3, 5, 7, 1_200), hmsm(3, 5, 6, 800), Duration::milliseconds(1400));

        // additional equality: `time1 + duration = time2` is equivalent to
        // `time2 - time1 = duration` IF AND ONLY IF `time2` represents a non-leap second.
        assert_eq!(hmsm(3, 5, 6, 800) + Duration::milliseconds(400), hmsm(3, 5, 7, 200));
        assert_eq!(hmsm(3, 5, 6, 1_800) + Duration::milliseconds(400), hmsm(3, 5, 7, 200));
    }

    #[test]
    fn test_time_fmt() {
        assert_eq!(format!("{}", NaiveTime::from_hms_milli(23, 59, 59, 999)), "23:59:59.999");
        assert_eq!(format!("{}", NaiveTime::from_hms_milli(23, 59, 59, 1_000)), "23:59:60");
        assert_eq!(format!("{}", NaiveTime::from_hms_milli(23, 59, 59, 1_001)), "23:59:60.001");
        assert_eq!(format!("{}", NaiveTime::from_hms_micro(0, 0, 0, 43210)), "00:00:00.043210");
        assert_eq!(format!("{}", NaiveTime::from_hms_nano(0, 0, 0, 6543210)), "00:00:00.006543210");

        // the format specifier should have no effect on `NaiveTime`
        assert_eq!(format!("{:30}", NaiveTime::from_hms_milli(3, 5, 7, 9)), "03:05:07.009");
    }

    #[test]
    fn test_date_from_str() {
        // valid cases
        let valid = [
            "0:0:0",
            "0:0:0.0000000",
            "0:0:0.0000003",
            " 4 : 3 : 2.1 ",
            " 09:08:07 ",
            " 9:8:07 ",
            "23:59:60.373929310237",
        ];
        for &s in &valid {
            let d = match s.parse::<NaiveTime>() {
                Ok(d) => d,
                Err(e) => panic!("parsing `{}` has failed: {}", s, e),
            };
            let s_ = format!("{:?}", d);
            // `s` and `s_` may differ, but `s.parse()` and `s_.parse()` must be same
            let d_ = match s_.parse::<NaiveTime>() {
                Ok(d) => d,
                Err(e) => {
                    panic!("`{}` is parsed into `{:?}`, but reparsing that has failed: {}", s, d, e)
                }
            };
            assert!(
                d == d_,
                "`{}` is parsed into `{:?}`, but reparsed result \
                              `{:?}` does not match",
                s,
                d,
                d_
            );
        }

        // some invalid cases
        // since `ParseErrorKind` is private, all we can do is to check if there was an error
        assert!("".parse::<NaiveTime>().is_err());
        assert!("x".parse::<NaiveTime>().is_err());
        assert!("15".parse::<NaiveTime>().is_err());
        assert!("15:8".parse::<NaiveTime>().is_err());
        assert!("15:8:x".parse::<NaiveTime>().is_err());
        assert!("15:8:9x".parse::<NaiveTime>().is_err());
        assert!("23:59:61".parse::<NaiveTime>().is_err());
        assert!("12:34:56.x".parse::<NaiveTime>().is_err());
        assert!("12:34:56. 0".parse::<NaiveTime>().is_err());
    }

    #[test]
    fn test_time_parse_from_str() {
        let hms = |h, m, s| NaiveTime::from_hms(h, m, s);
        assert_eq!(
            NaiveTime::parse_from_str("2014-5-7T12:34:56+09:30", "%Y-%m-%dT%H:%M:%S%z"),
            Ok(hms(12, 34, 56))
        ); // ignore date and offset
        assert_eq!(NaiveTime::parse_from_str("PM 12:59", "%P %H:%M"), Ok(hms(12, 59, 0)));
        assert!(NaiveTime::parse_from_str("12:3456", "%H:%M:%S").is_err());
    }

    #[test]
    fn test_time_format() {
        let t = NaiveTime::from_hms_nano(3, 5, 7, 98765432);
        assert_eq!(t.format("%H,%k,%I,%l,%P,%p").to_string(), "03, 3,03, 3,am,AM");
        assert_eq!(t.format("%M").to_string(), "05");
        assert_eq!(t.format("%S,%f,%.f").to_string(), "07,098765432,.098765432");
        assert_eq!(t.format("%.3f,%.6f,%.9f").to_string(), ".098,.098765,.098765432");
        assert_eq!(t.format("%R").to_string(), "03:05");
        assert_eq!(t.format("%T,%X").to_string(), "03:05:07,03:05:07");
        assert_eq!(t.format("%r").to_string(), "03:05:07 AM");
        assert_eq!(t.format("%t%n%%%n%t").to_string(), "\t\n%\n\t");

        let t = NaiveTime::from_hms_micro(3, 5, 7, 432100);
        assert_eq!(t.format("%S,%f,%.f").to_string(), "07,432100000,.432100");
        assert_eq!(t.format("%.3f,%.6f,%.9f").to_string(), ".432,.432100,.432100000");

        let t = NaiveTime::from_hms_milli(3, 5, 7, 210);
        assert_eq!(t.format("%S,%f,%.f").to_string(), "07,210000000,.210");
        assert_eq!(t.format("%.3f,%.6f,%.9f").to_string(), ".210,.210000,.210000000");

        let t = NaiveTime::from_hms(3, 5, 7);
        assert_eq!(t.format("%S,%f,%.f").to_string(), "07,000000000,");
        assert_eq!(t.format("%.3f,%.6f,%.9f").to_string(), ".000,.000000,.000000000");

        // corner cases
        assert_eq!(NaiveTime::from_hms(13, 57, 9).format("%r").to_string(), "01:57:09 PM");
        assert_eq!(
            NaiveTime::from_hms_milli(23, 59, 59, 1_000).format("%X").to_string(),
            "23:59:60"
        );
    }
}
