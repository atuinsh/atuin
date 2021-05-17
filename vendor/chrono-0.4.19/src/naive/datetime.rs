// This is a part of Chrono.
// See README.md and LICENSE.txt for details.

//! ISO 8601 date and time without timezone.

#[cfg(any(feature = "alloc", feature = "std", test))]
use core::borrow::Borrow;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::{fmt, hash, str};
use num_traits::ToPrimitive;
use oldtime::Duration as OldDuration;

use div::div_mod_floor;
#[cfg(any(feature = "alloc", feature = "std", test))]
use format::DelayedFormat;
use format::{parse, ParseError, ParseResult, Parsed, StrftimeItems};
use format::{Fixed, Item, Numeric, Pad};
use naive::date::{MAX_DATE, MIN_DATE};
use naive::time::{MAX_TIME, MIN_TIME};
use naive::{IsoWeek, NaiveDate, NaiveTime};
use {Datelike, Timelike, Weekday};

/// The tight upper bound guarantees that a duration with `|Duration| >= 2^MAX_SECS_BITS`
/// will always overflow the addition with any date and time type.
///
/// So why is this needed? `Duration::seconds(rhs)` may overflow, and we don't have
/// an alternative returning `Option` or `Result`. Thus we need some early bound to avoid
/// touching that call when we are already sure that it WILL overflow...
const MAX_SECS_BITS: usize = 44;

/// The minimum possible `NaiveDateTime`.
pub const MIN_DATETIME: NaiveDateTime = NaiveDateTime { date: MIN_DATE, time: MIN_TIME };
/// The maximum possible `NaiveDateTime`.
pub const MAX_DATETIME: NaiveDateTime = NaiveDateTime { date: MAX_DATE, time: MAX_TIME };

/// ISO 8601 combined date and time without timezone.
///
/// # Example
///
/// `NaiveDateTime` is commonly created from [`NaiveDate`](./struct.NaiveDate.html).
///
/// ~~~~
/// use chrono::{NaiveDate, NaiveDateTime};
///
/// let dt: NaiveDateTime = NaiveDate::from_ymd(2016, 7, 8).and_hms(9, 10, 11);
/// # let _ = dt;
/// ~~~~
///
/// You can use typical [date-like](../trait.Datelike.html) and
/// [time-like](../trait.Timelike.html) methods,
/// provided that relevant traits are in the scope.
///
/// ~~~~
/// # use chrono::{NaiveDate, NaiveDateTime};
/// # let dt: NaiveDateTime = NaiveDate::from_ymd(2016, 7, 8).and_hms(9, 10, 11);
/// use chrono::{Datelike, Timelike, Weekday};
///
/// assert_eq!(dt.weekday(), Weekday::Fri);
/// assert_eq!(dt.num_seconds_from_midnight(), 33011);
/// ~~~~
#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub struct NaiveDateTime {
    date: NaiveDate,
    time: NaiveTime,
}

impl NaiveDateTime {
    /// Makes a new `NaiveDateTime` from date and time components.
    /// Equivalent to [`date.and_time(time)`](./struct.NaiveDate.html#method.and_time)
    /// and many other helper constructors on `NaiveDate`.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveTime, NaiveDateTime};
    ///
    /// let d = NaiveDate::from_ymd(2015, 6, 3);
    /// let t = NaiveTime::from_hms_milli(12, 34, 56, 789);
    ///
    /// let dt = NaiveDateTime::new(d, t);
    /// assert_eq!(dt.date(), d);
    /// assert_eq!(dt.time(), t);
    /// ~~~~
    #[inline]
    pub fn new(date: NaiveDate, time: NaiveTime) -> NaiveDateTime {
        NaiveDateTime { date: date, time: time }
    }

    /// Makes a new `NaiveDateTime` corresponding to a UTC date and time,
    /// from the number of non-leap seconds
    /// since the midnight UTC on January 1, 1970 (aka "UNIX timestamp")
    /// and the number of nanoseconds since the last whole non-leap second.
    ///
    /// For a non-naive version of this function see
    /// [`TimeZone::timestamp`](../offset/trait.TimeZone.html#method.timestamp).
    ///
    /// The nanosecond part can exceed 1,000,000,000 in order to represent the
    /// [leap second](./struct.NaiveTime.html#leap-second-handling). (The true "UNIX
    /// timestamp" cannot represent a leap second unambiguously.)
    ///
    /// Panics on the out-of-range number of seconds and/or invalid nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDateTime, NaiveDate};
    ///
    /// let dt = NaiveDateTime::from_timestamp(0, 42_000_000);
    /// assert_eq!(dt, NaiveDate::from_ymd(1970, 1, 1).and_hms_milli(0, 0, 0, 42));
    ///
    /// let dt = NaiveDateTime::from_timestamp(1_000_000_000, 0);
    /// assert_eq!(dt, NaiveDate::from_ymd(2001, 9, 9).and_hms(1, 46, 40));
    /// ~~~~
    #[inline]
    pub fn from_timestamp(secs: i64, nsecs: u32) -> NaiveDateTime {
        let datetime = NaiveDateTime::from_timestamp_opt(secs, nsecs);
        datetime.expect("invalid or out-of-range datetime")
    }

    /// Makes a new `NaiveDateTime` corresponding to a UTC date and time,
    /// from the number of non-leap seconds
    /// since the midnight UTC on January 1, 1970 (aka "UNIX timestamp")
    /// and the number of nanoseconds since the last whole non-leap second.
    ///
    /// The nanosecond part can exceed 1,000,000,000
    /// in order to represent the [leap second](./struct.NaiveTime.html#leap-second-handling).
    /// (The true "UNIX timestamp" cannot represent a leap second unambiguously.)
    ///
    /// Returns `None` on the out-of-range number of seconds and/or invalid nanosecond.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDateTime, NaiveDate};
    /// use std::i64;
    ///
    /// let from_timestamp_opt = NaiveDateTime::from_timestamp_opt;
    ///
    /// assert!(from_timestamp_opt(0, 0).is_some());
    /// assert!(from_timestamp_opt(0, 999_999_999).is_some());
    /// assert!(from_timestamp_opt(0, 1_500_000_000).is_some()); // leap second
    /// assert!(from_timestamp_opt(0, 2_000_000_000).is_none());
    /// assert!(from_timestamp_opt(i64::MAX, 0).is_none());
    /// ~~~~
    #[inline]
    pub fn from_timestamp_opt(secs: i64, nsecs: u32) -> Option<NaiveDateTime> {
        let (days, secs) = div_mod_floor(secs, 86_400);
        let date = days
            .to_i32()
            .and_then(|days| days.checked_add(719_163))
            .and_then(NaiveDate::from_num_days_from_ce_opt);
        let time = NaiveTime::from_num_seconds_from_midnight_opt(secs as u32, nsecs);
        match (date, time) {
            (Some(date), Some(time)) => Some(NaiveDateTime { date: date, time: time }),
            (_, _) => None,
        }
    }

    /// Parses a string with the specified format string and returns a new `NaiveDateTime`.
    /// See the [`format::strftime` module](../format/strftime/index.html)
    /// on the supported escape sequences.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDateTime, NaiveDate};
    ///
    /// let parse_from_str = NaiveDateTime::parse_from_str;
    ///
    /// assert_eq!(parse_from_str("2015-09-05 23:56:04", "%Y-%m-%d %H:%M:%S"),
    ///            Ok(NaiveDate::from_ymd(2015, 9, 5).and_hms(23, 56, 4)));
    /// assert_eq!(parse_from_str("5sep2015pm012345.6789", "%d%b%Y%p%I%M%S%.f"),
    ///            Ok(NaiveDate::from_ymd(2015, 9, 5).and_hms_micro(13, 23, 45, 678_900)));
    /// ~~~~
    ///
    /// Offset is ignored for the purpose of parsing.
    ///
    /// ~~~~
    /// # use chrono::{NaiveDateTime, NaiveDate};
    /// # let parse_from_str = NaiveDateTime::parse_from_str;
    /// assert_eq!(parse_from_str("2014-5-17T12:34:56+09:30", "%Y-%m-%dT%H:%M:%S%z"),
    ///            Ok(NaiveDate::from_ymd(2014, 5, 17).and_hms(12, 34, 56)));
    /// ~~~~
    ///
    /// [Leap seconds](./struct.NaiveTime.html#leap-second-handling) are correctly handled by
    /// treating any time of the form `hh:mm:60` as a leap second.
    /// (This equally applies to the formatting, so the round trip is possible.)
    ///
    /// ~~~~
    /// # use chrono::{NaiveDateTime, NaiveDate};
    /// # let parse_from_str = NaiveDateTime::parse_from_str;
    /// assert_eq!(parse_from_str("2015-07-01 08:59:60.123", "%Y-%m-%d %H:%M:%S%.f"),
    ///            Ok(NaiveDate::from_ymd(2015, 7, 1).and_hms_milli(8, 59, 59, 1_123)));
    /// ~~~~
    ///
    /// Missing seconds are assumed to be zero,
    /// but out-of-bound times or insufficient fields are errors otherwise.
    ///
    /// ~~~~
    /// # use chrono::{NaiveDateTime, NaiveDate};
    /// # let parse_from_str = NaiveDateTime::parse_from_str;
    /// assert_eq!(parse_from_str("94/9/4 7:15", "%y/%m/%d %H:%M"),
    ///            Ok(NaiveDate::from_ymd(1994, 9, 4).and_hms(7, 15, 0)));
    ///
    /// assert!(parse_from_str("04m33s", "%Mm%Ss").is_err());
    /// assert!(parse_from_str("94/9/4 12", "%y/%m/%d %H").is_err());
    /// assert!(parse_from_str("94/9/4 17:60", "%y/%m/%d %H:%M").is_err());
    /// assert!(parse_from_str("94/9/4 24:00:00", "%y/%m/%d %H:%M:%S").is_err());
    /// ~~~~
    ///
    /// All parsed fields should be consistent to each other, otherwise it's an error.
    ///
    /// ~~~~
    /// # use chrono::NaiveDateTime;
    /// # let parse_from_str = NaiveDateTime::parse_from_str;
    /// let fmt = "%Y-%m-%d %H:%M:%S = UNIX timestamp %s";
    /// assert!(parse_from_str("2001-09-09 01:46:39 = UNIX timestamp 999999999", fmt).is_ok());
    /// assert!(parse_from_str("1970-01-01 00:00:00 = UNIX timestamp 1", fmt).is_err());
    /// ~~~~
    pub fn parse_from_str(s: &str, fmt: &str) -> ParseResult<NaiveDateTime> {
        let mut parsed = Parsed::new();
        parse(&mut parsed, s, StrftimeItems::new(fmt))?;
        parsed.to_naive_datetime_with_offset(0) // no offset adjustment
    }

    /// Retrieves a date component.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms(9, 10, 11);
    /// assert_eq!(dt.date(), NaiveDate::from_ymd(2016, 7, 8));
    /// ~~~~
    #[inline]
    pub fn date(&self) -> NaiveDate {
        self.date
    }

    /// Retrieves a time component.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveTime};
    ///
    /// let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms(9, 10, 11);
    /// assert_eq!(dt.time(), NaiveTime::from_hms(9, 10, 11));
    /// ~~~~
    #[inline]
    pub fn time(&self) -> NaiveTime {
        self.time
    }

    /// Returns the number of non-leap seconds since the midnight on January 1, 1970.
    ///
    /// Note that this does *not* account for the timezone!
    /// The true "UNIX timestamp" would count seconds since the midnight *UTC* on the epoch.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 980);
    /// assert_eq!(dt.timestamp(), 1);
    ///
    /// let dt = NaiveDate::from_ymd(2001, 9, 9).and_hms(1, 46, 40);
    /// assert_eq!(dt.timestamp(), 1_000_000_000);
    ///
    /// let dt = NaiveDate::from_ymd(1969, 12, 31).and_hms(23, 59, 59);
    /// assert_eq!(dt.timestamp(), -1);
    ///
    /// let dt = NaiveDate::from_ymd(-1, 1, 1).and_hms(0, 0, 0);
    /// assert_eq!(dt.timestamp(), -62198755200);
    /// ~~~~
    #[inline]
    pub fn timestamp(&self) -> i64 {
        const UNIX_EPOCH_DAY: i64 = 719_163;
        let gregorian_day = i64::from(self.date.num_days_from_ce());
        let seconds_from_midnight = i64::from(self.time.num_seconds_from_midnight());
        (gregorian_day - UNIX_EPOCH_DAY) * 86_400 + seconds_from_midnight
    }

    /// Returns the number of non-leap *milliseconds* since midnight on January 1, 1970.
    ///
    /// Note that this does *not* account for the timezone!
    /// The true "UNIX timestamp" would count seconds since the midnight *UTC* on the epoch.
    ///
    /// Note also that this does reduce the number of years that can be
    /// represented from ~584 Billion to ~584 Million. (If this is a problem,
    /// please file an issue to let me know what domain needs millisecond
    /// precision over billions of years, I'm curious.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 444);
    /// assert_eq!(dt.timestamp_millis(), 1_444);
    ///
    /// let dt = NaiveDate::from_ymd(2001, 9, 9).and_hms_milli(1, 46, 40, 555);
    /// assert_eq!(dt.timestamp_millis(), 1_000_000_000_555);
    ///
    /// let dt = NaiveDate::from_ymd(1969, 12, 31).and_hms_milli(23, 59, 59, 100);
    /// assert_eq!(dt.timestamp_millis(), -900);
    /// ~~~~
    #[inline]
    pub fn timestamp_millis(&self) -> i64 {
        let as_ms = self.timestamp() * 1000;
        as_ms + i64::from(self.timestamp_subsec_millis())
    }

    /// Returns the number of non-leap *nanoseconds* since midnight on January 1, 1970.
    ///
    /// Note that this does *not* account for the timezone!
    /// The true "UNIX timestamp" would count seconds since the midnight *UTC* on the epoch.
    ///
    /// # Panics
    ///
    /// Note also that this does reduce the number of years that can be
    /// represented from ~584 Billion to ~584 years. The dates that can be
    /// represented as nanoseconds are between 1677-09-21T00:12:44.0 and
    /// 2262-04-11T23:47:16.854775804.
    ///
    /// (If this is a problem, please file an issue to let me know what domain
    /// needs nanosecond precision over millennia, I'm curious.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime};
    ///
    /// let dt = NaiveDate::from_ymd(1970, 1, 1).and_hms_nano(0, 0, 1, 444);
    /// assert_eq!(dt.timestamp_nanos(), 1_000_000_444);
    ///
    /// let dt = NaiveDate::from_ymd(2001, 9, 9).and_hms_nano(1, 46, 40, 555);
    ///
    /// const A_BILLION: i64 = 1_000_000_000;
    /// let nanos = dt.timestamp_nanos();
    /// assert_eq!(nanos, 1_000_000_000_000_000_555);
    /// assert_eq!(
    ///     dt,
    ///     NaiveDateTime::from_timestamp(nanos / A_BILLION, (nanos % A_BILLION) as u32)
    /// );
    /// ~~~~
    #[inline]
    pub fn timestamp_nanos(&self) -> i64 {
        let as_ns = self.timestamp() * 1_000_000_000;
        as_ns + i64::from(self.timestamp_subsec_nanos())
    }

    /// Returns the number of milliseconds since the last whole non-leap second.
    ///
    /// The return value ranges from 0 to 999,
    /// or for [leap seconds](./struct.NaiveTime.html#leap-second-handling), to 1,999.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms_nano(9, 10, 11, 123_456_789);
    /// assert_eq!(dt.timestamp_subsec_millis(), 123);
    ///
    /// let dt = NaiveDate::from_ymd(2015, 7, 1).and_hms_nano(8, 59, 59, 1_234_567_890);
    /// assert_eq!(dt.timestamp_subsec_millis(), 1_234);
    /// ~~~~
    #[inline]
    pub fn timestamp_subsec_millis(&self) -> u32 {
        self.timestamp_subsec_nanos() / 1_000_000
    }

    /// Returns the number of microseconds since the last whole non-leap second.
    ///
    /// The return value ranges from 0 to 999,999,
    /// or for [leap seconds](./struct.NaiveTime.html#leap-second-handling), to 1,999,999.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms_nano(9, 10, 11, 123_456_789);
    /// assert_eq!(dt.timestamp_subsec_micros(), 123_456);
    ///
    /// let dt = NaiveDate::from_ymd(2015, 7, 1).and_hms_nano(8, 59, 59, 1_234_567_890);
    /// assert_eq!(dt.timestamp_subsec_micros(), 1_234_567);
    /// ~~~~
    #[inline]
    pub fn timestamp_subsec_micros(&self) -> u32 {
        self.timestamp_subsec_nanos() / 1_000
    }

    /// Returns the number of nanoseconds since the last whole non-leap second.
    ///
    /// The return value ranges from 0 to 999,999,999,
    /// or for [leap seconds](./struct.NaiveTime.html#leap-second-handling), to 1,999,999,999.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms_nano(9, 10, 11, 123_456_789);
    /// assert_eq!(dt.timestamp_subsec_nanos(), 123_456_789);
    ///
    /// let dt = NaiveDate::from_ymd(2015, 7, 1).and_hms_nano(8, 59, 59, 1_234_567_890);
    /// assert_eq!(dt.timestamp_subsec_nanos(), 1_234_567_890);
    /// ~~~~
    #[inline]
    pub fn timestamp_subsec_nanos(&self) -> u32 {
        self.time.nanosecond()
    }

    /// Adds given `Duration` to the current date and time.
    ///
    /// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
    /// the addition assumes that **there is no leap second ever**,
    /// except when the `NaiveDateTime` itself represents a leap second
    /// in which case the assumption becomes that **there is exactly a single leap second ever**.
    ///
    /// Returns `None` when it will result in overflow.
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveDate};
    ///
    /// let from_ymd = NaiveDate::from_ymd;
    ///
    /// let d = from_ymd(2016, 7, 8);
    /// let hms = |h, m, s| d.and_hms(h, m, s);
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::zero()),
    ///            Some(hms(3, 5, 7)));
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::seconds(1)),
    ///            Some(hms(3, 5, 8)));
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::seconds(-1)),
    ///            Some(hms(3, 5, 6)));
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::seconds(3600 + 60)),
    ///            Some(hms(4, 6, 7)));
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::seconds(86_400)),
    ///            Some(from_ymd(2016, 7, 9).and_hms(3, 5, 7)));
    ///
    /// let hmsm = |h, m, s, milli| d.and_hms_milli(h, m, s, milli);
    /// assert_eq!(hmsm(3, 5, 7, 980).checked_add_signed(Duration::milliseconds(450)),
    ///            Some(hmsm(3, 5, 8, 430)));
    /// # }
    /// ~~~~
    ///
    /// Overflow returns `None`.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveDate};
    /// # let hms = |h, m, s| NaiveDate::from_ymd(2016, 7, 8).and_hms(h, m, s);
    /// assert_eq!(hms(3, 5, 7).checked_add_signed(Duration::days(1_000_000_000)), None);
    /// # }
    /// ~~~~
    ///
    /// Leap seconds are handled,
    /// but the addition assumes that it is the only leap second happened.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveDate};
    /// # let from_ymd = NaiveDate::from_ymd;
    /// # let hmsm = |h, m, s, milli| from_ymd(2016, 7, 8).and_hms_milli(h, m, s, milli);
    /// let leap = hmsm(3, 5, 59, 1_300);
    /// assert_eq!(leap.checked_add_signed(Duration::zero()),
    ///            Some(hmsm(3, 5, 59, 1_300)));
    /// assert_eq!(leap.checked_add_signed(Duration::milliseconds(-500)),
    ///            Some(hmsm(3, 5, 59, 800)));
    /// assert_eq!(leap.checked_add_signed(Duration::milliseconds(500)),
    ///            Some(hmsm(3, 5, 59, 1_800)));
    /// assert_eq!(leap.checked_add_signed(Duration::milliseconds(800)),
    ///            Some(hmsm(3, 6, 0, 100)));
    /// assert_eq!(leap.checked_add_signed(Duration::seconds(10)),
    ///            Some(hmsm(3, 6, 9, 300)));
    /// assert_eq!(leap.checked_add_signed(Duration::seconds(-10)),
    ///            Some(hmsm(3, 5, 50, 300)));
    /// assert_eq!(leap.checked_add_signed(Duration::days(1)),
    ///            Some(from_ymd(2016, 7, 9).and_hms_milli(3, 5, 59, 300)));
    /// # }
    /// ~~~~
    pub fn checked_add_signed(self, rhs: OldDuration) -> Option<NaiveDateTime> {
        let (time, rhs) = self.time.overflowing_add_signed(rhs);

        // early checking to avoid overflow in OldDuration::seconds
        if rhs <= (-1 << MAX_SECS_BITS) || rhs >= (1 << MAX_SECS_BITS) {
            return None;
        }

        let date = try_opt!(self.date.checked_add_signed(OldDuration::seconds(rhs)));
        Some(NaiveDateTime { date: date, time: time })
    }

    /// Subtracts given `Duration` from the current date and time.
    ///
    /// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
    /// the subtraction assumes that **there is no leap second ever**,
    /// except when the `NaiveDateTime` itself represents a leap second
    /// in which case the assumption becomes that **there is exactly a single leap second ever**.
    ///
    /// Returns `None` when it will result in overflow.
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveDate};
    ///
    /// let from_ymd = NaiveDate::from_ymd;
    ///
    /// let d = from_ymd(2016, 7, 8);
    /// let hms = |h, m, s| d.and_hms(h, m, s);
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::zero()),
    ///            Some(hms(3, 5, 7)));
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::seconds(1)),
    ///            Some(hms(3, 5, 6)));
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::seconds(-1)),
    ///            Some(hms(3, 5, 8)));
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::seconds(3600 + 60)),
    ///            Some(hms(2, 4, 7)));
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::seconds(86_400)),
    ///            Some(from_ymd(2016, 7, 7).and_hms(3, 5, 7)));
    ///
    /// let hmsm = |h, m, s, milli| d.and_hms_milli(h, m, s, milli);
    /// assert_eq!(hmsm(3, 5, 7, 450).checked_sub_signed(Duration::milliseconds(670)),
    ///            Some(hmsm(3, 5, 6, 780)));
    /// # }
    /// ~~~~
    ///
    /// Overflow returns `None`.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveDate};
    /// # let hms = |h, m, s| NaiveDate::from_ymd(2016, 7, 8).and_hms(h, m, s);
    /// assert_eq!(hms(3, 5, 7).checked_sub_signed(Duration::days(1_000_000_000)), None);
    /// # }
    /// ~~~~
    ///
    /// Leap seconds are handled,
    /// but the subtraction assumes that it is the only leap second happened.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveDate};
    /// # let from_ymd = NaiveDate::from_ymd;
    /// # let hmsm = |h, m, s, milli| from_ymd(2016, 7, 8).and_hms_milli(h, m, s, milli);
    /// let leap = hmsm(3, 5, 59, 1_300);
    /// assert_eq!(leap.checked_sub_signed(Duration::zero()),
    ///            Some(hmsm(3, 5, 59, 1_300)));
    /// assert_eq!(leap.checked_sub_signed(Duration::milliseconds(200)),
    ///            Some(hmsm(3, 5, 59, 1_100)));
    /// assert_eq!(leap.checked_sub_signed(Duration::milliseconds(500)),
    ///            Some(hmsm(3, 5, 59, 800)));
    /// assert_eq!(leap.checked_sub_signed(Duration::seconds(60)),
    ///            Some(hmsm(3, 5, 0, 300)));
    /// assert_eq!(leap.checked_sub_signed(Duration::days(1)),
    ///            Some(from_ymd(2016, 7, 7).and_hms_milli(3, 6, 0, 300)));
    /// # }
    /// ~~~~
    pub fn checked_sub_signed(self, rhs: OldDuration) -> Option<NaiveDateTime> {
        let (time, rhs) = self.time.overflowing_sub_signed(rhs);

        // early checking to avoid overflow in OldDuration::seconds
        if rhs <= (-1 << MAX_SECS_BITS) || rhs >= (1 << MAX_SECS_BITS) {
            return None;
        }

        let date = try_opt!(self.date.checked_sub_signed(OldDuration::seconds(rhs)));
        Some(NaiveDateTime { date: date, time: time })
    }

    /// Subtracts another `NaiveDateTime` from the current date and time.
    /// This does not overflow or underflow at all.
    ///
    /// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
    /// the subtraction assumes that **there is no leap second ever**,
    /// except when any of the `NaiveDateTime`s themselves represents a leap second
    /// in which case the assumption becomes that
    /// **there are exactly one (or two) leap second(s) ever**.
    ///
    /// # Example
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// use chrono::{Duration, NaiveDate};
    ///
    /// let from_ymd = NaiveDate::from_ymd;
    ///
    /// let d = from_ymd(2016, 7, 8);
    /// assert_eq!(d.and_hms(3, 5, 7).signed_duration_since(d.and_hms(2, 4, 6)),
    ///            Duration::seconds(3600 + 60 + 1));
    ///
    /// // July 8 is 190th day in the year 2016
    /// let d0 = from_ymd(2016, 1, 1);
    /// assert_eq!(d.and_hms_milli(0, 7, 6, 500).signed_duration_since(d0.and_hms(0, 0, 0)),
    ///            Duration::seconds(189 * 86_400 + 7 * 60 + 6) + Duration::milliseconds(500));
    /// # }
    /// ~~~~
    ///
    /// Leap seconds are handled, but the subtraction assumes that
    /// there were no other leap seconds happened.
    ///
    /// ~~~~
    /// # extern crate chrono; fn main() {
    /// # use chrono::{Duration, NaiveDate};
    /// # let from_ymd = NaiveDate::from_ymd;
    /// let leap = from_ymd(2015, 6, 30).and_hms_milli(23, 59, 59, 1_500);
    /// assert_eq!(leap.signed_duration_since(from_ymd(2015, 6, 30).and_hms(23, 0, 0)),
    ///            Duration::seconds(3600) + Duration::milliseconds(500));
    /// assert_eq!(from_ymd(2015, 7, 1).and_hms(1, 0, 0).signed_duration_since(leap),
    ///            Duration::seconds(3600) - Duration::milliseconds(500));
    /// # }
    /// ~~~~
    pub fn signed_duration_since(self, rhs: NaiveDateTime) -> OldDuration {
        self.date.signed_duration_since(rhs.date) + self.time.signed_duration_since(rhs.time)
    }

    /// Formats the combined date and time with the specified formatting items.
    /// Otherwise it is the same as the ordinary [`format`](#method.format) method.
    ///
    /// The `Iterator` of items should be `Clone`able,
    /// since the resulting `DelayedFormat` value may be formatted multiple times.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::NaiveDate;
    /// use chrono::format::strftime::StrftimeItems;
    ///
    /// let fmt = StrftimeItems::new("%Y-%m-%d %H:%M:%S");
    /// let dt = NaiveDate::from_ymd(2015, 9, 5).and_hms(23, 56, 4);
    /// assert_eq!(dt.format_with_items(fmt.clone()).to_string(), "2015-09-05 23:56:04");
    /// assert_eq!(dt.format("%Y-%m-%d %H:%M:%S").to_string(),    "2015-09-05 23:56:04");
    /// ~~~~
    ///
    /// The resulting `DelayedFormat` can be formatted directly via the `Display` trait.
    ///
    /// ~~~~
    /// # use chrono::NaiveDate;
    /// # use chrono::format::strftime::StrftimeItems;
    /// # let fmt = StrftimeItems::new("%Y-%m-%d %H:%M:%S").clone();
    /// # let dt = NaiveDate::from_ymd(2015, 9, 5).and_hms(23, 56, 4);
    /// assert_eq!(format!("{}", dt.format_with_items(fmt)), "2015-09-05 23:56:04");
    /// ~~~~
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format_with_items<'a, I, B>(&self, items: I) -> DelayedFormat<I>
    where
        I: Iterator<Item = B> + Clone,
        B: Borrow<Item<'a>>,
    {
        DelayedFormat::new(Some(self.date), Some(self.time), items)
    }

    /// Formats the combined date and time with the specified format string.
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
    /// use chrono::NaiveDate;
    ///
    /// let dt = NaiveDate::from_ymd(2015, 9, 5).and_hms(23, 56, 4);
    /// assert_eq!(dt.format("%Y-%m-%d %H:%M:%S").to_string(), "2015-09-05 23:56:04");
    /// assert_eq!(dt.format("around %l %p on %b %-d").to_string(), "around 11 PM on Sep 5");
    /// ~~~~
    ///
    /// The resulting `DelayedFormat` can be formatted directly via the `Display` trait.
    ///
    /// ~~~~
    /// # use chrono::NaiveDate;
    /// # let dt = NaiveDate::from_ymd(2015, 9, 5).and_hms(23, 56, 4);
    /// assert_eq!(format!("{}", dt.format("%Y-%m-%d %H:%M:%S")), "2015-09-05 23:56:04");
    /// assert_eq!(format!("{}", dt.format("around %l %p on %b %-d")), "around 11 PM on Sep 5");
    /// ~~~~
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.format_with_items(StrftimeItems::new(fmt))
    }
}

impl Datelike for NaiveDateTime {
    /// Returns the year number in the [calendar date](./index.html#calendar-date).
    ///
    /// See also the [`NaiveDate::year`](./struct.NaiveDate.html#method.year) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.year(), 2015);
    /// ~~~~
    #[inline]
    fn year(&self) -> i32 {
        self.date.year()
    }

    /// Returns the month number starting from 1.
    ///
    /// The return value ranges from 1 to 12.
    ///
    /// See also the [`NaiveDate::month`](./struct.NaiveDate.html#method.month) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.month(), 9);
    /// ~~~~
    #[inline]
    fn month(&self) -> u32 {
        self.date.month()
    }

    /// Returns the month number starting from 0.
    ///
    /// The return value ranges from 0 to 11.
    ///
    /// See also the [`NaiveDate::month0`](./struct.NaiveDate.html#method.month0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.month0(), 8);
    /// ~~~~
    #[inline]
    fn month0(&self) -> u32 {
        self.date.month0()
    }

    /// Returns the day of month starting from 1.
    ///
    /// The return value ranges from 1 to 31. (The last day of month differs by months.)
    ///
    /// See also the [`NaiveDate::day`](./struct.NaiveDate.html#method.day) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.day(), 25);
    /// ~~~~
    #[inline]
    fn day(&self) -> u32 {
        self.date.day()
    }

    /// Returns the day of month starting from 0.
    ///
    /// The return value ranges from 0 to 30. (The last day of month differs by months.)
    ///
    /// See also the [`NaiveDate::day0`](./struct.NaiveDate.html#method.day0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.day0(), 24);
    /// ~~~~
    #[inline]
    fn day0(&self) -> u32 {
        self.date.day0()
    }

    /// Returns the day of year starting from 1.
    ///
    /// The return value ranges from 1 to 366. (The last day of year differs by years.)
    ///
    /// See also the [`NaiveDate::ordinal`](./struct.NaiveDate.html#method.ordinal) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.ordinal(), 268);
    /// ~~~~
    #[inline]
    fn ordinal(&self) -> u32 {
        self.date.ordinal()
    }

    /// Returns the day of year starting from 0.
    ///
    /// The return value ranges from 0 to 365. (The last day of year differs by years.)
    ///
    /// See also the [`NaiveDate::ordinal0`](./struct.NaiveDate.html#method.ordinal0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.ordinal0(), 267);
    /// ~~~~
    #[inline]
    fn ordinal0(&self) -> u32 {
        self.date.ordinal0()
    }

    /// Returns the day of week.
    ///
    /// See also the [`NaiveDate::weekday`](./struct.NaiveDate.html#method.weekday) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike, Weekday};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.weekday(), Weekday::Fri);
    /// ~~~~
    #[inline]
    fn weekday(&self) -> Weekday {
        self.date.weekday()
    }

    #[inline]
    fn iso_week(&self) -> IsoWeek {
        self.date.iso_week()
    }

    /// Makes a new `NaiveDateTime` with the year number changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_year`](./struct.NaiveDate.html#method.with_year) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 25).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_year(2016), Some(NaiveDate::from_ymd(2016, 9, 25).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_year(-308), Some(NaiveDate::from_ymd(-308, 9, 25).and_hms(12, 34, 56)));
    /// ~~~~
    #[inline]
    fn with_year(&self, year: i32) -> Option<NaiveDateTime> {
        self.date.with_year(year).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the month number (starting from 1) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_month`](./struct.NaiveDate.html#method.with_month) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 30).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_month(10), Some(NaiveDate::from_ymd(2015, 10, 30).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_month(13), None); // no month 13
    /// assert_eq!(dt.with_month(2), None); // no February 30
    /// ~~~~
    #[inline]
    fn with_month(&self, month: u32) -> Option<NaiveDateTime> {
        self.date.with_month(month).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the month number (starting from 0) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_month0`](./struct.NaiveDate.html#method.with_month0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 30).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_month0(9), Some(NaiveDate::from_ymd(2015, 10, 30).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_month0(12), None); // no month 13
    /// assert_eq!(dt.with_month0(1), None); // no February 30
    /// ~~~~
    #[inline]
    fn with_month0(&self, month0: u32) -> Option<NaiveDateTime> {
        self.date.with_month0(month0).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the day of month (starting from 1) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_day`](./struct.NaiveDate.html#method.with_day) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_day(30), Some(NaiveDate::from_ymd(2015, 9, 30).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_day(31), None); // no September 31
    /// ~~~~
    #[inline]
    fn with_day(&self, day: u32) -> Option<NaiveDateTime> {
        self.date.with_day(day).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the day of month (starting from 0) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_day0`](./struct.NaiveDate.html#method.with_day0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_day0(29), Some(NaiveDate::from_ymd(2015, 9, 30).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_day0(30), None); // no September 31
    /// ~~~~
    #[inline]
    fn with_day0(&self, day0: u32) -> Option<NaiveDateTime> {
        self.date.with_day0(day0).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the day of year (starting from 1) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_ordinal`](./struct.NaiveDate.html#method.with_ordinal) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_ordinal(60),
    ///            Some(NaiveDate::from_ymd(2015, 3, 1).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_ordinal(366), None); // 2015 had only 365 days
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2016, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_ordinal(60),
    ///            Some(NaiveDate::from_ymd(2016, 2, 29).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_ordinal(366),
    ///            Some(NaiveDate::from_ymd(2016, 12, 31).and_hms(12, 34, 56)));
    /// ~~~~
    #[inline]
    fn with_ordinal(&self, ordinal: u32) -> Option<NaiveDateTime> {
        self.date.with_ordinal(ordinal).map(|d| NaiveDateTime { date: d, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the day of year (starting from 0) changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveDate::with_ordinal0`](./struct.NaiveDate.html#method.with_ordinal0) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Datelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_ordinal0(59),
    ///            Some(NaiveDate::from_ymd(2015, 3, 1).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_ordinal0(365), None); // 2015 had only 365 days
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2016, 9, 8).and_hms(12, 34, 56);
    /// assert_eq!(dt.with_ordinal0(59),
    ///            Some(NaiveDate::from_ymd(2016, 2, 29).and_hms(12, 34, 56)));
    /// assert_eq!(dt.with_ordinal0(365),
    ///            Some(NaiveDate::from_ymd(2016, 12, 31).and_hms(12, 34, 56)));
    /// ~~~~
    #[inline]
    fn with_ordinal0(&self, ordinal0: u32) -> Option<NaiveDateTime> {
        self.date.with_ordinal0(ordinal0).map(|d| NaiveDateTime { date: d, ..*self })
    }
}

impl Timelike for NaiveDateTime {
    /// Returns the hour number from 0 to 23.
    ///
    /// See also the [`NaiveTime::hour`](./struct.NaiveTime.html#method.hour) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.hour(), 12);
    /// ~~~~
    #[inline]
    fn hour(&self) -> u32 {
        self.time.hour()
    }

    /// Returns the minute number from 0 to 59.
    ///
    /// See also the [`NaiveTime::minute`](./struct.NaiveTime.html#method.minute) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.minute(), 34);
    /// ~~~~
    #[inline]
    fn minute(&self) -> u32 {
        self.time.minute()
    }

    /// Returns the second number from 0 to 59.
    ///
    /// See also the [`NaiveTime::second`](./struct.NaiveTime.html#method.second) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.second(), 56);
    /// ~~~~
    #[inline]
    fn second(&self) -> u32 {
        self.time.second()
    }

    /// Returns the number of nanoseconds since the whole non-leap second.
    /// The range from 1,000,000,000 to 1,999,999,999 represents
    /// the [leap second](./struct.NaiveTime.html#leap-second-handling).
    ///
    /// See also the
    /// [`NaiveTime::nanosecond`](./struct.NaiveTime.html#method.nanosecond) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.nanosecond(), 789_000_000);
    /// ~~~~
    #[inline]
    fn nanosecond(&self) -> u32 {
        self.time.nanosecond()
    }

    /// Makes a new `NaiveDateTime` with the hour number changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveTime::with_hour`](./struct.NaiveTime.html#method.with_hour) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.with_hour(7),
    ///            Some(NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(7, 34, 56, 789)));
    /// assert_eq!(dt.with_hour(24), None);
    /// ~~~~
    #[inline]
    fn with_hour(&self, hour: u32) -> Option<NaiveDateTime> {
        self.time.with_hour(hour).map(|t| NaiveDateTime { time: t, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the minute number changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    ///
    /// See also the
    /// [`NaiveTime::with_minute`](./struct.NaiveTime.html#method.with_minute) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.with_minute(45),
    ///            Some(NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 45, 56, 789)));
    /// assert_eq!(dt.with_minute(60), None);
    /// ~~~~
    #[inline]
    fn with_minute(&self, min: u32) -> Option<NaiveDateTime> {
        self.time.with_minute(min).map(|t| NaiveDateTime { time: t, ..*self })
    }

    /// Makes a new `NaiveDateTime` with the second number changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    /// As with the [`second`](#method.second) method,
    /// the input range is restricted to 0 through 59.
    ///
    /// See also the
    /// [`NaiveTime::with_second`](./struct.NaiveTime.html#method.with_second) method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.with_second(17),
    ///            Some(NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 17, 789)));
    /// assert_eq!(dt.with_second(60), None);
    /// ~~~~
    #[inline]
    fn with_second(&self, sec: u32) -> Option<NaiveDateTime> {
        self.time.with_second(sec).map(|t| NaiveDateTime { time: t, ..*self })
    }

    /// Makes a new `NaiveDateTime` with nanoseconds since the whole non-leap second changed.
    ///
    /// Returns `None` when the resulting `NaiveDateTime` would be invalid.
    /// As with the [`nanosecond`](#method.nanosecond) method,
    /// the input range can exceed 1,000,000,000 for leap seconds.
    ///
    /// See also the
    /// [`NaiveTime::with_nanosecond`](./struct.NaiveTime.html#method.with_nanosecond)
    /// method.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{NaiveDate, NaiveDateTime, Timelike};
    ///
    /// let dt: NaiveDateTime = NaiveDate::from_ymd(2015, 9, 8).and_hms_milli(12, 34, 56, 789);
    /// assert_eq!(dt.with_nanosecond(333_333_333),
    ///            Some(NaiveDate::from_ymd(2015, 9, 8).and_hms_nano(12, 34, 56, 333_333_333)));
    /// assert_eq!(dt.with_nanosecond(1_333_333_333), // leap second
    ///            Some(NaiveDate::from_ymd(2015, 9, 8).and_hms_nano(12, 34, 56, 1_333_333_333)));
    /// assert_eq!(dt.with_nanosecond(2_000_000_000), None);
    /// ~~~~
    #[inline]
    fn with_nanosecond(&self, nano: u32) -> Option<NaiveDateTime> {
        self.time.with_nanosecond(nano).map(|t| NaiveDateTime { time: t, ..*self })
    }
}

/// `NaiveDateTime` can be used as a key to the hash maps (in principle).
///
/// Practically this also takes account of fractional seconds, so it is not recommended.
/// (For the obvious reason this also distinguishes leap seconds from non-leap seconds.)
impl hash::Hash for NaiveDateTime {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.date.hash(state);
        self.time.hash(state);
    }
}

/// An addition of `Duration` to `NaiveDateTime` yields another `NaiveDateTime`.
///
/// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
/// the addition assumes that **there is no leap second ever**,
/// except when the `NaiveDateTime` itself represents a leap second
/// in which case the assumption becomes that **there is exactly a single leap second ever**.
///
/// Panics on underflow or overflow.
/// Use [`NaiveDateTime::checked_add_signed`](#method.checked_add_signed) to detect that.
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveDate};
///
/// let from_ymd = NaiveDate::from_ymd;
///
/// let d = from_ymd(2016, 7, 8);
/// let hms = |h, m, s| d.and_hms(h, m, s);
/// assert_eq!(hms(3, 5, 7) + Duration::zero(),             hms(3, 5, 7));
/// assert_eq!(hms(3, 5, 7) + Duration::seconds(1),         hms(3, 5, 8));
/// assert_eq!(hms(3, 5, 7) + Duration::seconds(-1),        hms(3, 5, 6));
/// assert_eq!(hms(3, 5, 7) + Duration::seconds(3600 + 60), hms(4, 6, 7));
/// assert_eq!(hms(3, 5, 7) + Duration::seconds(86_400),
///            from_ymd(2016, 7, 9).and_hms(3, 5, 7));
/// assert_eq!(hms(3, 5, 7) + Duration::days(365),
///            from_ymd(2017, 7, 8).and_hms(3, 5, 7));
///
/// let hmsm = |h, m, s, milli| d.and_hms_milli(h, m, s, milli);
/// assert_eq!(hmsm(3, 5, 7, 980) + Duration::milliseconds(450), hmsm(3, 5, 8, 430));
/// # }
/// ~~~~
///
/// Leap seconds are handled,
/// but the addition assumes that it is the only leap second happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveDate};
/// # let from_ymd = NaiveDate::from_ymd;
/// # let hmsm = |h, m, s, milli| from_ymd(2016, 7, 8).and_hms_milli(h, m, s, milli);
/// let leap = hmsm(3, 5, 59, 1_300);
/// assert_eq!(leap + Duration::zero(),             hmsm(3, 5, 59, 1_300));
/// assert_eq!(leap + Duration::milliseconds(-500), hmsm(3, 5, 59, 800));
/// assert_eq!(leap + Duration::milliseconds(500),  hmsm(3, 5, 59, 1_800));
/// assert_eq!(leap + Duration::milliseconds(800),  hmsm(3, 6, 0, 100));
/// assert_eq!(leap + Duration::seconds(10),        hmsm(3, 6, 9, 300));
/// assert_eq!(leap + Duration::seconds(-10),       hmsm(3, 5, 50, 300));
/// assert_eq!(leap + Duration::days(1),
///            from_ymd(2016, 7, 9).and_hms_milli(3, 5, 59, 300));
/// # }
/// ~~~~
impl Add<OldDuration> for NaiveDateTime {
    type Output = NaiveDateTime;

    #[inline]
    fn add(self, rhs: OldDuration) -> NaiveDateTime {
        self.checked_add_signed(rhs).expect("`NaiveDateTime + Duration` overflowed")
    }
}

impl AddAssign<OldDuration> for NaiveDateTime {
    #[inline]
    fn add_assign(&mut self, rhs: OldDuration) {
        *self = self.add(rhs);
    }
}

/// A subtraction of `Duration` from `NaiveDateTime` yields another `NaiveDateTime`.
/// It is the same as the addition with a negated `Duration`.
///
/// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
/// the addition assumes that **there is no leap second ever**,
/// except when the `NaiveDateTime` itself represents a leap second
/// in which case the assumption becomes that **there is exactly a single leap second ever**.
///
/// Panics on underflow or overflow.
/// Use [`NaiveDateTime::checked_sub_signed`](#method.checked_sub_signed) to detect that.
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveDate};
///
/// let from_ymd = NaiveDate::from_ymd;
///
/// let d = from_ymd(2016, 7, 8);
/// let hms = |h, m, s| d.and_hms(h, m, s);
/// assert_eq!(hms(3, 5, 7) - Duration::zero(),             hms(3, 5, 7));
/// assert_eq!(hms(3, 5, 7) - Duration::seconds(1),         hms(3, 5, 6));
/// assert_eq!(hms(3, 5, 7) - Duration::seconds(-1),        hms(3, 5, 8));
/// assert_eq!(hms(3, 5, 7) - Duration::seconds(3600 + 60), hms(2, 4, 7));
/// assert_eq!(hms(3, 5, 7) - Duration::seconds(86_400),
///            from_ymd(2016, 7, 7).and_hms(3, 5, 7));
/// assert_eq!(hms(3, 5, 7) - Duration::days(365),
///            from_ymd(2015, 7, 9).and_hms(3, 5, 7));
///
/// let hmsm = |h, m, s, milli| d.and_hms_milli(h, m, s, milli);
/// assert_eq!(hmsm(3, 5, 7, 450) - Duration::milliseconds(670), hmsm(3, 5, 6, 780));
/// # }
/// ~~~~
///
/// Leap seconds are handled,
/// but the subtraction assumes that it is the only leap second happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveDate};
/// # let from_ymd = NaiveDate::from_ymd;
/// # let hmsm = |h, m, s, milli| from_ymd(2016, 7, 8).and_hms_milli(h, m, s, milli);
/// let leap = hmsm(3, 5, 59, 1_300);
/// assert_eq!(leap - Duration::zero(),            hmsm(3, 5, 59, 1_300));
/// assert_eq!(leap - Duration::milliseconds(200), hmsm(3, 5, 59, 1_100));
/// assert_eq!(leap - Duration::milliseconds(500), hmsm(3, 5, 59, 800));
/// assert_eq!(leap - Duration::seconds(60),       hmsm(3, 5, 0, 300));
/// assert_eq!(leap - Duration::days(1),
///            from_ymd(2016, 7, 7).and_hms_milli(3, 6, 0, 300));
/// # }
/// ~~~~
impl Sub<OldDuration> for NaiveDateTime {
    type Output = NaiveDateTime;

    #[inline]
    fn sub(self, rhs: OldDuration) -> NaiveDateTime {
        self.checked_sub_signed(rhs).expect("`NaiveDateTime - Duration` overflowed")
    }
}

impl SubAssign<OldDuration> for NaiveDateTime {
    #[inline]
    fn sub_assign(&mut self, rhs: OldDuration) {
        *self = self.sub(rhs);
    }
}

/// Subtracts another `NaiveDateTime` from the current date and time.
/// This does not overflow or underflow at all.
///
/// As a part of Chrono's [leap second handling](./struct.NaiveTime.html#leap-second-handling),
/// the subtraction assumes that **there is no leap second ever**,
/// except when any of the `NaiveDateTime`s themselves represents a leap second
/// in which case the assumption becomes that
/// **there are exactly one (or two) leap second(s) ever**.
///
/// The implementation is a wrapper around
/// [`NaiveDateTime::signed_duration_since`](#method.signed_duration_since).
///
/// # Example
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// use chrono::{Duration, NaiveDate};
///
/// let from_ymd = NaiveDate::from_ymd;
///
/// let d = from_ymd(2016, 7, 8);
/// assert_eq!(d.and_hms(3, 5, 7) - d.and_hms(2, 4, 6), Duration::seconds(3600 + 60 + 1));
///
/// // July 8 is 190th day in the year 2016
/// let d0 = from_ymd(2016, 1, 1);
/// assert_eq!(d.and_hms_milli(0, 7, 6, 500) - d0.and_hms(0, 0, 0),
///            Duration::seconds(189 * 86_400 + 7 * 60 + 6) + Duration::milliseconds(500));
/// # }
/// ~~~~
///
/// Leap seconds are handled, but the subtraction assumes that
/// there were no other leap seconds happened.
///
/// ~~~~
/// # extern crate chrono; fn main() {
/// # use chrono::{Duration, NaiveDate};
/// # let from_ymd = NaiveDate::from_ymd;
/// let leap = from_ymd(2015, 6, 30).and_hms_milli(23, 59, 59, 1_500);
/// assert_eq!(leap - from_ymd(2015, 6, 30).and_hms(23, 0, 0),
///            Duration::seconds(3600) + Duration::milliseconds(500));
/// assert_eq!(from_ymd(2015, 7, 1).and_hms(1, 0, 0) - leap,
///            Duration::seconds(3600) - Duration::milliseconds(500));
/// # }
/// ~~~~
impl Sub<NaiveDateTime> for NaiveDateTime {
    type Output = OldDuration;

    #[inline]
    fn sub(self, rhs: NaiveDateTime) -> OldDuration {
        self.signed_duration_since(rhs)
    }
}

/// The `Debug` output of the naive date and time `dt` is the same as
/// [`dt.format("%Y-%m-%dT%H:%M:%S%.f")`](../format/strftime/index.html).
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
/// use chrono::NaiveDate;
///
/// let dt = NaiveDate::from_ymd(2016, 11, 15).and_hms(7, 39, 24);
/// assert_eq!(format!("{:?}", dt), "2016-11-15T07:39:24");
/// ~~~~
///
/// Leap seconds may also be used.
///
/// ~~~~
/// # use chrono::NaiveDate;
/// let dt = NaiveDate::from_ymd(2015, 6, 30).and_hms_milli(23, 59, 59, 1_500);
/// assert_eq!(format!("{:?}", dt), "2015-06-30T23:59:60.500");
/// ~~~~
impl fmt::Debug for NaiveDateTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}T{:?}", self.date, self.time)
    }
}

/// The `Display` output of the naive date and time `dt` is the same as
/// [`dt.format("%Y-%m-%d %H:%M:%S%.f")`](../format/strftime/index.html).
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
/// use chrono::NaiveDate;
///
/// let dt = NaiveDate::from_ymd(2016, 11, 15).and_hms(7, 39, 24);
/// assert_eq!(format!("{}", dt), "2016-11-15 07:39:24");
/// ~~~~
///
/// Leap seconds may also be used.
///
/// ~~~~
/// # use chrono::NaiveDate;
/// let dt = NaiveDate::from_ymd(2015, 6, 30).and_hms_milli(23, 59, 59, 1_500);
/// assert_eq!(format!("{}", dt), "2015-06-30 23:59:60.500");
/// ~~~~
impl fmt::Display for NaiveDateTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.date, self.time)
    }
}

/// Parsing a `str` into a `NaiveDateTime` uses the same format,
/// [`%Y-%m-%dT%H:%M:%S%.f`](../format/strftime/index.html), as in `Debug`.
///
/// # Example
///
/// ~~~~
/// use chrono::{NaiveDateTime, NaiveDate};
///
/// let dt = NaiveDate::from_ymd(2015, 9, 18).and_hms(23, 56, 4);
/// assert_eq!("2015-09-18T23:56:04".parse::<NaiveDateTime>(), Ok(dt));
///
/// let dt = NaiveDate::from_ymd(12345, 6, 7).and_hms_milli(7, 59, 59, 1_500); // leap second
/// assert_eq!("+12345-6-7T7:59:60.5".parse::<NaiveDateTime>(), Ok(dt));
///
/// assert!("foo".parse::<NaiveDateTime>().is_err());
/// ~~~~
impl str::FromStr for NaiveDateTime {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<NaiveDateTime> {
        const ITEMS: &'static [Item<'static>] = &[
            Item::Numeric(Numeric::Year, Pad::Zero),
            Item::Space(""),
            Item::Literal("-"),
            Item::Numeric(Numeric::Month, Pad::Zero),
            Item::Space(""),
            Item::Literal("-"),
            Item::Numeric(Numeric::Day, Pad::Zero),
            Item::Space(""),
            Item::Literal("T"), // XXX shouldn't this be case-insensitive?
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
        parsed.to_naive_datetime_with_offset(0)
    }
}

#[cfg(all(test, any(feature = "rustc-serialize", feature = "serde")))]
fn test_encodable_json<F, E>(to_string: F)
where
    F: Fn(&NaiveDateTime) -> Result<String, E>,
    E: ::std::fmt::Debug,
{
    use naive::{MAX_DATE, MIN_DATE};

    assert_eq!(
        to_string(&NaiveDate::from_ymd(2016, 7, 8).and_hms_milli(9, 10, 48, 90)).ok(),
        Some(r#""2016-07-08T09:10:48.090""#.into())
    );
    assert_eq!(
        to_string(&NaiveDate::from_ymd(2014, 7, 24).and_hms(12, 34, 6)).ok(),
        Some(r#""2014-07-24T12:34:06""#.into())
    );
    assert_eq!(
        to_string(&NaiveDate::from_ymd(0, 1, 1).and_hms_milli(0, 0, 59, 1_000)).ok(),
        Some(r#""0000-01-01T00:00:60""#.into())
    );
    assert_eq!(
        to_string(&NaiveDate::from_ymd(-1, 12, 31).and_hms_nano(23, 59, 59, 7)).ok(),
        Some(r#""-0001-12-31T23:59:59.000000007""#.into())
    );
    assert_eq!(
        to_string(&MIN_DATE.and_hms(0, 0, 0)).ok(),
        Some(r#""-262144-01-01T00:00:00""#.into())
    );
    assert_eq!(
        to_string(&MAX_DATE.and_hms_nano(23, 59, 59, 1_999_999_999)).ok(),
        Some(r#""+262143-12-31T23:59:60.999999999""#.into())
    );
}

#[cfg(all(test, any(feature = "rustc-serialize", feature = "serde")))]
fn test_decodable_json<F, E>(from_str: F)
where
    F: Fn(&str) -> Result<NaiveDateTime, E>,
    E: ::std::fmt::Debug,
{
    use naive::{MAX_DATE, MIN_DATE};

    assert_eq!(
        from_str(r#""2016-07-08T09:10:48.090""#).ok(),
        Some(NaiveDate::from_ymd(2016, 7, 8).and_hms_milli(9, 10, 48, 90))
    );
    assert_eq!(
        from_str(r#""2016-7-8T9:10:48.09""#).ok(),
        Some(NaiveDate::from_ymd(2016, 7, 8).and_hms_milli(9, 10, 48, 90))
    );
    assert_eq!(
        from_str(r#""2014-07-24T12:34:06""#).ok(),
        Some(NaiveDate::from_ymd(2014, 7, 24).and_hms(12, 34, 6))
    );
    assert_eq!(
        from_str(r#""0000-01-01T00:00:60""#).ok(),
        Some(NaiveDate::from_ymd(0, 1, 1).and_hms_milli(0, 0, 59, 1_000))
    );
    assert_eq!(
        from_str(r#""0-1-1T0:0:60""#).ok(),
        Some(NaiveDate::from_ymd(0, 1, 1).and_hms_milli(0, 0, 59, 1_000))
    );
    assert_eq!(
        from_str(r#""-0001-12-31T23:59:59.000000007""#).ok(),
        Some(NaiveDate::from_ymd(-1, 12, 31).and_hms_nano(23, 59, 59, 7))
    );
    assert_eq!(from_str(r#""-262144-01-01T00:00:00""#).ok(), Some(MIN_DATE.and_hms(0, 0, 0)));
    assert_eq!(
        from_str(r#""+262143-12-31T23:59:60.999999999""#).ok(),
        Some(MAX_DATE.and_hms_nano(23, 59, 59, 1_999_999_999))
    );
    assert_eq!(
        from_str(r#""+262143-12-31T23:59:60.9999999999997""#).ok(), // excess digits are ignored
        Some(MAX_DATE.and_hms_nano(23, 59, 59, 1_999_999_999))
    );

    // bad formats
    assert!(from_str(r#""""#).is_err());
    assert!(from_str(r#""2016-07-08""#).is_err());
    assert!(from_str(r#""09:10:48.090""#).is_err());
    assert!(from_str(r#""20160708T091048.090""#).is_err());
    assert!(from_str(r#""2000-00-00T00:00:00""#).is_err());
    assert!(from_str(r#""2000-02-30T00:00:00""#).is_err());
    assert!(from_str(r#""2001-02-29T00:00:00""#).is_err());
    assert!(from_str(r#""2002-02-28T24:00:00""#).is_err());
    assert!(from_str(r#""2002-02-28T23:60:00""#).is_err());
    assert!(from_str(r#""2002-02-28T23:59:61""#).is_err());
    assert!(from_str(r#""2016-07-08T09:10:48,090""#).is_err());
    assert!(from_str(r#""2016-07-08 09:10:48.090""#).is_err());
    assert!(from_str(r#""2016-007-08T09:10:48.090""#).is_err());
    assert!(from_str(r#""yyyy-mm-ddThh:mm:ss.fffffffff""#).is_err());
    assert!(from_str(r#"20160708000000"#).is_err());
    assert!(from_str(r#"{}"#).is_err());
    // pre-0.3.0 rustc-serialize format is now invalid
    assert!(from_str(r#"{"date":{"ymdf":20},"time":{"secs":0,"frac":0}}"#).is_err());
    assert!(from_str(r#"null"#).is_err());
}

#[cfg(all(test, feature = "rustc-serialize"))]
fn test_decodable_json_timestamp<F, E>(from_str: F)
where
    F: Fn(&str) -> Result<rustc_serialize::TsSeconds, E>,
    E: ::std::fmt::Debug,
{
    assert_eq!(
        *from_str("0").unwrap(),
        NaiveDate::from_ymd(1970, 1, 1).and_hms(0, 0, 0),
        "should parse integers as timestamps"
    );
    assert_eq!(
        *from_str("-1").unwrap(),
        NaiveDate::from_ymd(1969, 12, 31).and_hms(23, 59, 59),
        "should parse integers as timestamps"
    );
}

#[cfg(feature = "rustc-serialize")]
pub mod rustc_serialize {
    use super::NaiveDateTime;
    use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};
    use std::ops::Deref;

    impl Encodable for NaiveDateTime {
        fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
            format!("{:?}", self).encode(s)
        }
    }

    impl Decodable for NaiveDateTime {
        fn decode<D: Decoder>(d: &mut D) -> Result<NaiveDateTime, D::Error> {
            d.read_str()?.parse().map_err(|_| d.error("invalid date time string"))
        }
    }

    /// A `DateTime` that can be deserialized from a seconds-based timestamp
    #[derive(Debug)]
    #[deprecated(
        since = "1.4.2",
        note = "RustcSerialize will be removed before chrono 1.0, use Serde instead"
    )]
    pub struct TsSeconds(NaiveDateTime);

    #[allow(deprecated)]
    impl From<TsSeconds> for NaiveDateTime {
        /// Pull the internal NaiveDateTime out
        #[allow(deprecated)]
        fn from(obj: TsSeconds) -> NaiveDateTime {
            obj.0
        }
    }

    #[allow(deprecated)]
    impl Deref for TsSeconds {
        type Target = NaiveDateTime;

        #[allow(deprecated)]
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    #[allow(deprecated)]
    impl Decodable for TsSeconds {
        #[allow(deprecated)]
        fn decode<D: Decoder>(d: &mut D) -> Result<TsSeconds, D::Error> {
            Ok(TsSeconds(
                NaiveDateTime::from_timestamp_opt(d.read_i64()?, 0)
                    .ok_or_else(|| d.error("invalid timestamp"))?,
            ))
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

    #[test]
    fn test_decodable_timestamps() {
        super::test_decodable_json_timestamp(json::decode);
    }
}

/// Tools to help serializing/deserializing `NaiveDateTime`s
#[cfg(feature = "serde")]
pub mod serde {
    use super::NaiveDateTime;
    use core::fmt;
    use serdelib::{de, ser};

    /// Serialize a `NaiveDateTime` as an RFC 3339 string
    ///
    /// See [the `serde` module](./serde/index.html) for alternate
    /// serialization formats.
    impl ser::Serialize for NaiveDateTime {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            struct FormatWrapped<'a, D: 'a> {
                inner: &'a D,
            }

            impl<'a, D: fmt::Debug> fmt::Display for FormatWrapped<'a, D> {
                fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    self.inner.fmt(f)
                }
            }

            serializer.collect_str(&FormatWrapped { inner: &self })
        }
    }

    struct NaiveDateTimeVisitor;

    impl<'de> de::Visitor<'de> for NaiveDateTimeVisitor {
        type Value = NaiveDateTime;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a formatted date and time string")
        }

        fn visit_str<E>(self, value: &str) -> Result<NaiveDateTime, E>
        where
            E: de::Error,
        {
            value.parse().map_err(E::custom)
        }
    }

    impl<'de> de::Deserialize<'de> for NaiveDateTime {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_str(NaiveDateTimeVisitor)
        }
    }

    /// Used to serialize/deserialize from nanosecond-precision timestamps
    ///
    /// # Example:
    ///
    /// ```rust
    /// # // We mark this ignored so that we can test on 1.13 (which does not
    /// # // support custom derive), and run tests with --ignored on beta and
    /// # // nightly to actually trigger these.
    /// #
    /// # #[macro_use] extern crate serde_derive;
    /// # extern crate serde_json;
    /// # extern crate serde;
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
    /// use chrono::naive::serde::ts_nanoseconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_nanoseconds")]
    ///     time: NaiveDateTime
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = NaiveDate::from_ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733);
    /// let my_s = S {
    ///     time: time.clone(),
    /// };
    ///
    /// let as_string = serde_json::to_string(&my_s)?;
    /// assert_eq!(as_string, r#"{"time":1526522699918355733}"#);
    /// let my_s: S = serde_json::from_str(&as_string)?;
    /// assert_eq!(my_s.time, time);
    /// # Ok(my_s)
    /// # }
    /// # fn main() { example().unwrap(); }
    /// ```
    pub mod ts_nanoseconds {
        use core::fmt;
        use serdelib::{de, ser};

        use {ne_timestamp, NaiveDateTime};

        /// Serialize a UTC datetime into an integer number of nanoseconds since the epoch
        ///
        /// Intended for use with `serde`s `serialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # #[macro_use] extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
        /// # use serde::Serialize;
        /// use chrono::naive::serde::ts_nanoseconds::serialize as to_nano_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_nano_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: NaiveDate::from_ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918355733}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.serialize_i64(dt.timestamp_nanos())
        }

        /// Deserialize a `DateTime` from a nanoseconds timestamp
        ///
        /// Intended for use with `serde`s `deserialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{NaiveDateTime, Utc};
        /// # use serde::Deserialize;
        /// use chrono::naive::serde::ts_nanoseconds::deserialize as from_nano_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_nano_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1526522699918355733 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<NaiveDateTime, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(NaiveDateTimeFromNanoSecondsVisitor)?)
        }

        struct NaiveDateTimeFromNanoSecondsVisitor;

        impl<'de> de::Visitor<'de> for NaiveDateTimeFromNanoSecondsVisitor {
            type Value = NaiveDateTime;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp")
            }

            fn visit_i64<E>(self, value: i64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(
                    value / 1_000_000_000,
                    (value % 1_000_000_000) as u32,
                )
                .ok_or_else(|| E::custom(ne_timestamp(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(
                    value as i64 / 1_000_000_000,
                    (value as i64 % 1_000_000_000) as u32,
                )
                .ok_or_else(|| E::custom(ne_timestamp(value)))
            }
        }
    }

    /// Used to serialize/deserialize from millisecond-precision timestamps
    ///
    /// # Example:
    ///
    /// ```rust
    /// # // We mark this ignored so that we can test on 1.13 (which does not
    /// # // support custom derive), and run tests with --ignored on beta and
    /// # // nightly to actually trigger these.
    /// #
    /// # #[macro_use] extern crate serde_derive;
    /// # extern crate serde_json;
    /// # extern crate serde;
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
    /// use chrono::naive::serde::ts_milliseconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_milliseconds")]
    ///     time: NaiveDateTime
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = NaiveDate::from_ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918);
    /// let my_s = S {
    ///     time: time.clone(),
    /// };
    ///
    /// let as_string = serde_json::to_string(&my_s)?;
    /// assert_eq!(as_string, r#"{"time":1526522699918}"#);
    /// let my_s: S = serde_json::from_str(&as_string)?;
    /// assert_eq!(my_s.time, time);
    /// # Ok(my_s)
    /// # }
    /// # fn main() { example().unwrap(); }
    /// ```
    pub mod ts_milliseconds {
        use core::fmt;
        use serdelib::{de, ser};

        use {ne_timestamp, NaiveDateTime};

        /// Serialize a UTC datetime into an integer number of milliseconds since the epoch
        ///
        /// Intended for use with `serde`s `serialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # #[macro_use] extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
        /// # use serde::Serialize;
        /// use chrono::naive::serde::ts_milliseconds::serialize as to_milli_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_milli_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: NaiveDate::from_ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.serialize_i64(dt.timestamp_millis())
        }

        /// Deserialize a `DateTime` from a milliseconds timestamp
        ///
        /// Intended for use with `serde`s `deserialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{NaiveDateTime, Utc};
        /// # use serde::Deserialize;
        /// use chrono::naive::serde::ts_milliseconds::deserialize as from_milli_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_milli_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1526522699918 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<NaiveDateTime, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(NaiveDateTimeFromMilliSecondsVisitor)?)
        }

        struct NaiveDateTimeFromMilliSecondsVisitor;

        impl<'de> de::Visitor<'de> for NaiveDateTimeFromMilliSecondsVisitor {
            type Value = NaiveDateTime;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp")
            }

            fn visit_i64<E>(self, value: i64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(value / 1000, ((value % 1000) * 1_000_000) as u32)
                    .ok_or_else(|| E::custom(ne_timestamp(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(
                    (value / 1000) as i64,
                    ((value % 1000) * 1_000_000) as u32,
                )
                .ok_or_else(|| E::custom(ne_timestamp(value)))
            }
        }
    }

    /// Used to serialize/deserialize from second-precision timestamps
    ///
    /// # Example:
    ///
    /// ```rust
    /// # // We mark this ignored so that we can test on 1.13 (which does not
    /// # // support custom derive), and run tests with --ignored on beta and
    /// # // nightly to actually trigger these.
    /// #
    /// # #[macro_use] extern crate serde_derive;
    /// # extern crate serde_json;
    /// # extern crate serde;
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
    /// use chrono::naive::serde::ts_seconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_seconds")]
    ///     time: NaiveDateTime
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = NaiveDate::from_ymd(2015, 5, 15).and_hms(10, 0, 0);
    /// let my_s = S {
    ///     time: time.clone(),
    /// };
    ///
    /// let as_string = serde_json::to_string(&my_s)?;
    /// assert_eq!(as_string, r#"{"time":1431684000}"#);
    /// let my_s: S = serde_json::from_str(&as_string)?;
    /// assert_eq!(my_s.time, time);
    /// # Ok(my_s)
    /// # }
    /// # fn main() { example().unwrap(); }
    /// ```
    pub mod ts_seconds {
        use core::fmt;
        use serdelib::{de, ser};

        use {ne_timestamp, NaiveDateTime};

        /// Serialize a UTC datetime into an integer number of seconds since the epoch
        ///
        /// Intended for use with `serde`s `serialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # #[macro_use] extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, NaiveDate, NaiveDateTime, Utc};
        /// # use serde::Serialize;
        /// use chrono::naive::serde::ts_seconds::serialize as to_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: NaiveDate::from_ymd(2015, 5, 15).and_hms(10, 0, 0),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1431684000}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.serialize_i64(dt.timestamp())
        }

        /// Deserialize a `DateTime` from a seconds timestamp
        ///
        /// Intended for use with `serde`s `deserialize_with` attribute.
        ///
        /// # Example:
        ///
        /// ```rust
        /// # // We mark this ignored so that we can test on 1.13 (which does not
        /// # // support custom derive), and run tests with --ignored on beta and
        /// # // nightly to actually trigger these.
        /// #
        /// # #[macro_use] extern crate serde_derive;
        /// # #[macro_use] extern crate serde_json;
        /// # extern crate serde;
        /// # extern crate chrono;
        /// # use chrono::{NaiveDateTime, Utc};
        /// # use serde::Deserialize;
        /// use chrono::naive::serde::ts_seconds::deserialize as from_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_ts")]
        ///     time: NaiveDateTime
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1431684000 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<NaiveDateTime, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(NaiveDateTimeFromSecondsVisitor)?)
        }

        struct NaiveDateTimeFromSecondsVisitor;

        impl<'de> de::Visitor<'de> for NaiveDateTimeFromSecondsVisitor {
            type Value = NaiveDateTime;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp")
            }

            fn visit_i64<E>(self, value: i64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(value, 0)
                    .ok_or_else(|| E::custom(ne_timestamp(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<NaiveDateTime, E>
            where
                E: de::Error,
            {
                NaiveDateTime::from_timestamp_opt(value as i64, 0)
                    .ok_or_else(|| E::custom(ne_timestamp(value)))
            }
        }
    }

    #[cfg(test)]
    extern crate bincode;
    #[cfg(test)]
    extern crate serde_derive;
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

    // Bincode is relevant to test separately from JSON because
    // it is not self-describing.
    #[test]
    fn test_serde_bincode() {
        use self::bincode::{deserialize, serialize, Infinite};
        use naive::NaiveDate;

        let dt = NaiveDate::from_ymd(2016, 7, 8).and_hms_milli(9, 10, 48, 90);
        let encoded = serialize(&dt, Infinite).unwrap();
        let decoded: NaiveDateTime = deserialize(&encoded).unwrap();
        assert_eq!(dt, decoded);
    }

    #[test]
    fn test_serde_bincode_optional() {
        use self::bincode::{deserialize, serialize, Infinite};
        use self::serde_derive::{Deserialize, Serialize};
        use prelude::*;
        use serde::ts_nanoseconds_option;

        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Test {
            one: Option<i64>,
            #[serde(with = "ts_nanoseconds_option")]
            two: Option<DateTime<Utc>>,
        }

        let expected = Test { one: Some(1), two: Some(Utc.ymd(1970, 1, 1).and_hms(0, 1, 1)) };
        let bytes: Vec<u8> = serialize(&expected, Infinite).unwrap();
        let actual = deserialize::<Test>(&(bytes)).unwrap();

        assert_eq!(expected, actual);
    }
}

#[cfg(test)]
mod tests {
    use super::NaiveDateTime;
    use naive::{NaiveDate, MAX_DATE, MIN_DATE};
    use oldtime::Duration;
    use std::i64;
    use Datelike;

    #[test]
    fn test_datetime_from_timestamp() {
        let from_timestamp = |secs| NaiveDateTime::from_timestamp_opt(secs, 0);
        let ymdhms = |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
        assert_eq!(from_timestamp(-1), Some(ymdhms(1969, 12, 31, 23, 59, 59)));
        assert_eq!(from_timestamp(0), Some(ymdhms(1970, 1, 1, 0, 0, 0)));
        assert_eq!(from_timestamp(1), Some(ymdhms(1970, 1, 1, 0, 0, 1)));
        assert_eq!(from_timestamp(1_000_000_000), Some(ymdhms(2001, 9, 9, 1, 46, 40)));
        assert_eq!(from_timestamp(0x7fffffff), Some(ymdhms(2038, 1, 19, 3, 14, 7)));
        assert_eq!(from_timestamp(i64::MIN), None);
        assert_eq!(from_timestamp(i64::MAX), None);
    }

    #[test]
    fn test_datetime_add() {
        fn check(
            (y, m, d, h, n, s): (i32, u32, u32, u32, u32, u32),
            rhs: Duration,
            result: Option<(i32, u32, u32, u32, u32, u32)>,
        ) {
            let lhs = NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
            let sum =
                result.map(|(y, m, d, h, n, s)| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s));
            assert_eq!(lhs.checked_add_signed(rhs), sum);
            assert_eq!(lhs.checked_sub_signed(-rhs), sum);
        };

        check(
            (2014, 5, 6, 7, 8, 9),
            Duration::seconds(3600 + 60 + 1),
            Some((2014, 5, 6, 8, 9, 10)),
        );
        check(
            (2014, 5, 6, 7, 8, 9),
            Duration::seconds(-(3600 + 60 + 1)),
            Some((2014, 5, 6, 6, 7, 8)),
        );
        check((2014, 5, 6, 7, 8, 9), Duration::seconds(86399), Some((2014, 5, 7, 7, 8, 8)));
        check((2014, 5, 6, 7, 8, 9), Duration::seconds(86_400 * 10), Some((2014, 5, 16, 7, 8, 9)));
        check((2014, 5, 6, 7, 8, 9), Duration::seconds(-86_400 * 10), Some((2014, 4, 26, 7, 8, 9)));
        check((2014, 5, 6, 7, 8, 9), Duration::seconds(86_400 * 10), Some((2014, 5, 16, 7, 8, 9)));

        // overflow check
        // assumes that we have correct values for MAX/MIN_DAYS_FROM_YEAR_0 from `naive::date`.
        // (they are private constants, but the equivalence is tested in that module.)
        let max_days_from_year_0 = MAX_DATE.signed_duration_since(NaiveDate::from_ymd(0, 1, 1));
        check((0, 1, 1, 0, 0, 0), max_days_from_year_0, Some((MAX_DATE.year(), 12, 31, 0, 0, 0)));
        check(
            (0, 1, 1, 0, 0, 0),
            max_days_from_year_0 + Duration::seconds(86399),
            Some((MAX_DATE.year(), 12, 31, 23, 59, 59)),
        );
        check((0, 1, 1, 0, 0, 0), max_days_from_year_0 + Duration::seconds(86_400), None);
        check((0, 1, 1, 0, 0, 0), Duration::max_value(), None);

        let min_days_from_year_0 = MIN_DATE.signed_duration_since(NaiveDate::from_ymd(0, 1, 1));
        check((0, 1, 1, 0, 0, 0), min_days_from_year_0, Some((MIN_DATE.year(), 1, 1, 0, 0, 0)));
        check((0, 1, 1, 0, 0, 0), min_days_from_year_0 - Duration::seconds(1), None);
        check((0, 1, 1, 0, 0, 0), Duration::min_value(), None);
    }

    #[test]
    fn test_datetime_sub() {
        let ymdhms = |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
        let since = NaiveDateTime::signed_duration_since;
        assert_eq!(
            since(ymdhms(2014, 5, 6, 7, 8, 9), ymdhms(2014, 5, 6, 7, 8, 9)),
            Duration::zero()
        );
        assert_eq!(
            since(ymdhms(2014, 5, 6, 7, 8, 10), ymdhms(2014, 5, 6, 7, 8, 9)),
            Duration::seconds(1)
        );
        assert_eq!(
            since(ymdhms(2014, 5, 6, 7, 8, 9), ymdhms(2014, 5, 6, 7, 8, 10)),
            Duration::seconds(-1)
        );
        assert_eq!(
            since(ymdhms(2014, 5, 7, 7, 8, 9), ymdhms(2014, 5, 6, 7, 8, 10)),
            Duration::seconds(86399)
        );
        assert_eq!(
            since(ymdhms(2001, 9, 9, 1, 46, 39), ymdhms(1970, 1, 1, 0, 0, 0)),
            Duration::seconds(999_999_999)
        );
    }

    #[test]
    fn test_datetime_addassignment() {
        let ymdhms = |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
        let mut date = ymdhms(2016, 10, 1, 10, 10, 10);
        date += Duration::minutes(10_000_000);
        assert_eq!(date, ymdhms(2035, 10, 6, 20, 50, 10));
        date += Duration::days(10);
        assert_eq!(date, ymdhms(2035, 10, 16, 20, 50, 10));
    }

    #[test]
    fn test_datetime_subassignment() {
        let ymdhms = |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
        let mut date = ymdhms(2016, 10, 1, 10, 10, 10);
        date -= Duration::minutes(10_000_000);
        assert_eq!(date, ymdhms(1997, 9, 26, 23, 30, 10));
        date -= Duration::days(10);
        assert_eq!(date, ymdhms(1997, 9, 16, 23, 30, 10));
    }

    #[test]
    fn test_datetime_timestamp() {
        let to_timestamp =
            |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s).timestamp();
        assert_eq!(to_timestamp(1969, 12, 31, 23, 59, 59), -1);
        assert_eq!(to_timestamp(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(to_timestamp(1970, 1, 1, 0, 0, 1), 1);
        assert_eq!(to_timestamp(2001, 9, 9, 1, 46, 40), 1_000_000_000);
        assert_eq!(to_timestamp(2038, 1, 19, 3, 14, 7), 0x7fffffff);
    }

    #[test]
    fn test_datetime_from_str() {
        // valid cases
        let valid = [
            "2015-2-18T23:16:9.15",
            "-77-02-18T23:16:09",
            "  +82701  -  05  -  6  T  15  :  9  : 60.898989898989   ",
        ];
        for &s in &valid {
            let d = match s.parse::<NaiveDateTime>() {
                Ok(d) => d,
                Err(e) => panic!("parsing `{}` has failed: {}", s, e),
            };
            let s_ = format!("{:?}", d);
            // `s` and `s_` may differ, but `s.parse()` and `s_.parse()` must be same
            let d_ = match s_.parse::<NaiveDateTime>() {
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
        assert!("".parse::<NaiveDateTime>().is_err());
        assert!("x".parse::<NaiveDateTime>().is_err());
        assert!("15".parse::<NaiveDateTime>().is_err());
        assert!("15:8:9".parse::<NaiveDateTime>().is_err());
        assert!("15-8-9".parse::<NaiveDateTime>().is_err());
        assert!("2015-15-15T15:15:15".parse::<NaiveDateTime>().is_err());
        assert!("2012-12-12T12:12:12x".parse::<NaiveDateTime>().is_err());
        assert!("2012-123-12T12:12:12".parse::<NaiveDateTime>().is_err());
        assert!("+ 82701-123-12T12:12:12".parse::<NaiveDateTime>().is_err());
        assert!("+802701-123-12T12:12:12".parse::<NaiveDateTime>().is_err()); // out-of-bound
    }

    #[test]
    fn test_datetime_parse_from_str() {
        let ymdhms = |y, m, d, h, n, s| NaiveDate::from_ymd(y, m, d).and_hms(h, n, s);
        let ymdhmsn =
            |y, m, d, h, n, s, nano| NaiveDate::from_ymd(y, m, d).and_hms_nano(h, n, s, nano);
        assert_eq!(
            NaiveDateTime::parse_from_str("2014-5-7T12:34:56+09:30", "%Y-%m-%dT%H:%M:%S%z"),
            Ok(ymdhms(2014, 5, 7, 12, 34, 56))
        ); // ignore offset
        assert_eq!(
            NaiveDateTime::parse_from_str("2015-W06-1 000000", "%G-W%V-%u%H%M%S"),
            Ok(ymdhms(2015, 2, 2, 0, 0, 0))
        );
        assert_eq!(
            NaiveDateTime::parse_from_str(
                "Fri, 09 Aug 2013 23:54:35 GMT",
                "%a, %d %b %Y %H:%M:%S GMT"
            ),
            Ok(ymdhms(2013, 8, 9, 23, 54, 35))
        );
        assert!(NaiveDateTime::parse_from_str(
            "Sat, 09 Aug 2013 23:54:35 GMT",
            "%a, %d %b %Y %H:%M:%S GMT"
        )
        .is_err());
        assert!(NaiveDateTime::parse_from_str("2014-5-7 12:3456", "%Y-%m-%d %H:%M:%S").is_err());
        assert!(NaiveDateTime::parse_from_str("12:34:56", "%H:%M:%S").is_err()); // insufficient
        assert_eq!(
            NaiveDateTime::parse_from_str("1441497364", "%s"),
            Ok(ymdhms(2015, 9, 5, 23, 56, 4))
        );
        assert_eq!(
            NaiveDateTime::parse_from_str("1283929614.1234", "%s.%f"),
            Ok(ymdhmsn(2010, 9, 8, 7, 6, 54, 1234))
        );
        assert_eq!(
            NaiveDateTime::parse_from_str("1441497364.649", "%s%.3f"),
            Ok(ymdhmsn(2015, 9, 5, 23, 56, 4, 649000000))
        );
        assert_eq!(
            NaiveDateTime::parse_from_str("1497854303.087654", "%s%.6f"),
            Ok(ymdhmsn(2017, 6, 19, 6, 38, 23, 87654000))
        );
        assert_eq!(
            NaiveDateTime::parse_from_str("1437742189.918273645", "%s%.9f"),
            Ok(ymdhmsn(2015, 7, 24, 12, 49, 49, 918273645))
        );
    }

    #[test]
    fn test_datetime_format() {
        let dt = NaiveDate::from_ymd(2010, 9, 8).and_hms_milli(7, 6, 54, 321);
        assert_eq!(dt.format("%c").to_string(), "Wed Sep  8 07:06:54 2010");
        assert_eq!(dt.format("%s").to_string(), "1283929614");
        assert_eq!(dt.format("%t%n%%%n%t").to_string(), "\t\n%\n\t");

        // a horror of leap second: coming near to you.
        let dt = NaiveDate::from_ymd(2012, 6, 30).and_hms_milli(23, 59, 59, 1_000);
        assert_eq!(dt.format("%c").to_string(), "Sat Jun 30 23:59:60 2012");
        assert_eq!(dt.format("%s").to_string(), "1341100799"); // not 1341100800, it's intentional.
    }

    #[test]
    fn test_datetime_add_sub_invariant() {
        // issue #37
        let base = NaiveDate::from_ymd(2000, 1, 1).and_hms(0, 0, 0);
        let t = -946684799990000;
        let time = base + Duration::microseconds(t);
        assert_eq!(t, time.signed_duration_since(base).num_microseconds().unwrap());
    }

    #[test]
    fn test_nanosecond_range() {
        const A_BILLION: i64 = 1_000_000_000;
        let maximum = "2262-04-11T23:47:16.854775804";
        let parsed: NaiveDateTime = maximum.parse().unwrap();
        let nanos = parsed.timestamp_nanos();
        assert_eq!(
            parsed,
            NaiveDateTime::from_timestamp(nanos / A_BILLION, (nanos % A_BILLION) as u32)
        );

        let minimum = "1677-09-21T00:12:44.000000000";
        let parsed: NaiveDateTime = minimum.parse().unwrap();
        let nanos = parsed.timestamp_nanos();
        assert_eq!(
            parsed,
            NaiveDateTime::from_timestamp(nanos / A_BILLION, (nanos % A_BILLION) as u32)
        );
    }
}
