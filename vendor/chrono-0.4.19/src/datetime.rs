// This is a part of Chrono.
// See README.md and LICENSE.txt for details.

//! ISO 8601 date and time with time zone.

use core::cmp::Ordering;
use core::ops::{Add, Sub};
use core::{fmt, hash, str};
use oldtime::Duration as OldDuration;
#[cfg(any(feature = "std", test))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(all(not(feature = "std"), feature = "alloc"))]
use alloc::string::{String, ToString};
#[cfg(feature = "std")]
use std::string::ToString;

#[cfg(any(feature = "alloc", feature = "std", test))]
use core::borrow::Borrow;
#[cfg(any(feature = "alloc", feature = "std", test))]
use format::DelayedFormat;
#[cfg(feature = "unstable-locales")]
use format::Locale;
use format::{parse, ParseError, ParseResult, Parsed, StrftimeItems};
use format::{Fixed, Item};
use naive::{self, IsoWeek, NaiveDateTime, NaiveTime};
#[cfg(feature = "clock")]
use offset::Local;
use offset::{FixedOffset, Offset, TimeZone, Utc};
use Date;
use {Datelike, Timelike, Weekday};

/// Specific formatting options for seconds. This may be extended in the
/// future, so exhaustive matching in external code is not recommended.
///
/// See the `TimeZone::to_rfc3339_opts` function for usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecondsFormat {
    /// Format whole seconds only, with no decimal point nor subseconds.
    Secs,

    /// Use fixed 3 subsecond digits. This corresponds to
    /// [Fixed::Nanosecond3](format/enum.Fixed.html#variant.Nanosecond3).
    Millis,

    /// Use fixed 6 subsecond digits. This corresponds to
    /// [Fixed::Nanosecond6](format/enum.Fixed.html#variant.Nanosecond6).
    Micros,

    /// Use fixed 9 subsecond digits. This corresponds to
    /// [Fixed::Nanosecond9](format/enum.Fixed.html#variant.Nanosecond9).
    Nanos,

    /// Automatically select one of `Secs`, `Millis`, `Micros`, or `Nanos` to
    /// display all available non-zero sub-second digits.  This corresponds to
    /// [Fixed::Nanosecond](format/enum.Fixed.html#variant.Nanosecond).
    AutoSi,

    // Do not match against this.
    #[doc(hidden)]
    __NonExhaustive,
}

/// ISO 8601 combined date and time with time zone.
///
/// There are some constructors implemented here (the `from_*` methods), but
/// the general-purpose constructors are all via the methods on the
/// [`TimeZone`](./offset/trait.TimeZone.html) implementations.
#[derive(Clone)]
pub struct DateTime<Tz: TimeZone> {
    datetime: NaiveDateTime,
    offset: Tz::Offset,
}

/// The minimum possible `DateTime<Utc>`.
pub const MIN_DATETIME: DateTime<Utc> = DateTime { datetime: naive::MIN_DATETIME, offset: Utc };
/// The maximum possible `DateTime<Utc>`.
pub const MAX_DATETIME: DateTime<Utc> = DateTime { datetime: naive::MAX_DATETIME, offset: Utc };

impl<Tz: TimeZone> DateTime<Tz> {
    /// Makes a new `DateTime` with given *UTC* datetime and offset.
    /// The local datetime should be constructed via the `TimeZone` trait.
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::{DateTime, TimeZone, NaiveDateTime, Utc};
    ///
    /// let dt = DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(61, 0), Utc);
    /// assert_eq!(Utc.timestamp(61, 0), dt);
    /// ~~~~
    //
    // note: this constructor is purposely not named to `new` to discourage the direct usage.
    #[inline]
    pub fn from_utc(datetime: NaiveDateTime, offset: Tz::Offset) -> DateTime<Tz> {
        DateTime { datetime: datetime, offset: offset }
    }

    /// Retrieves a date component.
    #[inline]
    pub fn date(&self) -> Date<Tz> {
        Date::from_utc(self.naive_local().date(), self.offset.clone())
    }

    /// Retrieves a time component.
    /// Unlike `date`, this is not associated to the time zone.
    #[inline]
    pub fn time(&self) -> NaiveTime {
        self.datetime.time() + self.offset.fix()
    }

    /// Returns the number of non-leap seconds since January 1, 1970 0:00:00 UTC
    /// (aka "UNIX timestamp").
    #[inline]
    pub fn timestamp(&self) -> i64 {
        self.datetime.timestamp()
    }

    /// Returns the number of non-leap-milliseconds since January 1, 1970 UTC
    ///
    /// Note that this does reduce the number of years that can be represented
    /// from ~584 Billion to ~584 Million. (If this is a problem, please file
    /// an issue to let me know what domain needs millisecond precision over
    /// billions of years, I'm curious.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::Utc;
    /// use chrono::TimeZone;
    ///
    /// let dt = Utc.ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 444);
    /// assert_eq!(dt.timestamp_millis(), 1_444);
    ///
    /// let dt = Utc.ymd(2001, 9, 9).and_hms_milli(1, 46, 40, 555);
    /// assert_eq!(dt.timestamp_millis(), 1_000_000_000_555);
    /// ~~~~
    #[inline]
    pub fn timestamp_millis(&self) -> i64 {
        self.datetime.timestamp_millis()
    }

    /// Returns the number of non-leap-nanoseconds since January 1, 1970 UTC
    ///
    /// Note that this does reduce the number of years that can be represented
    /// from ~584 Billion to ~584. (If this is a problem, please file
    /// an issue to let me know what domain needs nanosecond precision over
    /// millennia, I'm curious.)
    ///
    /// # Example
    ///
    /// ~~~~
    /// use chrono::Utc;
    /// use chrono::TimeZone;
    ///
    /// let dt = Utc.ymd(1970, 1, 1).and_hms_nano(0, 0, 1, 444);
    /// assert_eq!(dt.timestamp_nanos(), 1_000_000_444);
    ///
    /// let dt = Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 40, 555);
    /// assert_eq!(dt.timestamp_nanos(), 1_000_000_000_000_000_555);
    /// ~~~~
    #[inline]
    pub fn timestamp_nanos(&self) -> i64 {
        self.datetime.timestamp_nanos()
    }

    /// Returns the number of milliseconds since the last second boundary
    ///
    /// warning: in event of a leap second, this may exceed 999
    ///
    /// note: this is not the number of milliseconds since January 1, 1970 0:00:00 UTC
    #[inline]
    pub fn timestamp_subsec_millis(&self) -> u32 {
        self.datetime.timestamp_subsec_millis()
    }

    /// Returns the number of microseconds since the last second boundary
    ///
    /// warning: in event of a leap second, this may exceed 999_999
    ///
    /// note: this is not the number of microseconds since January 1, 1970 0:00:00 UTC
    #[inline]
    pub fn timestamp_subsec_micros(&self) -> u32 {
        self.datetime.timestamp_subsec_micros()
    }

    /// Returns the number of nanoseconds since the last second boundary
    ///
    /// warning: in event of a leap second, this may exceed 999_999_999
    ///
    /// note: this is not the number of nanoseconds since January 1, 1970 0:00:00 UTC
    #[inline]
    pub fn timestamp_subsec_nanos(&self) -> u32 {
        self.datetime.timestamp_subsec_nanos()
    }

    /// Retrieves an associated offset from UTC.
    #[inline]
    pub fn offset(&self) -> &Tz::Offset {
        &self.offset
    }

    /// Retrieves an associated time zone.
    #[inline]
    pub fn timezone(&self) -> Tz {
        TimeZone::from_offset(&self.offset)
    }

    /// Changes the associated time zone.
    /// This does not change the actual `DateTime` (but will change the string representation).
    #[inline]
    pub fn with_timezone<Tz2: TimeZone>(&self, tz: &Tz2) -> DateTime<Tz2> {
        tz.from_utc_datetime(&self.datetime)
    }

    /// Adds given `Duration` to the current date and time.
    ///
    /// Returns `None` when it will result in overflow.
    #[inline]
    pub fn checked_add_signed(self, rhs: OldDuration) -> Option<DateTime<Tz>> {
        let datetime = try_opt!(self.datetime.checked_add_signed(rhs));
        let tz = self.timezone();
        Some(tz.from_utc_datetime(&datetime))
    }

    /// Subtracts given `Duration` from the current date and time.
    ///
    /// Returns `None` when it will result in overflow.
    #[inline]
    pub fn checked_sub_signed(self, rhs: OldDuration) -> Option<DateTime<Tz>> {
        let datetime = try_opt!(self.datetime.checked_sub_signed(rhs));
        let tz = self.timezone();
        Some(tz.from_utc_datetime(&datetime))
    }

    /// Subtracts another `DateTime` from the current date and time.
    /// This does not overflow or underflow at all.
    #[inline]
    pub fn signed_duration_since<Tz2: TimeZone>(self, rhs: DateTime<Tz2>) -> OldDuration {
        self.datetime.signed_duration_since(rhs.datetime)
    }

    /// Returns a view to the naive UTC datetime.
    #[inline]
    pub fn naive_utc(&self) -> NaiveDateTime {
        self.datetime
    }

    /// Returns a view to the naive local datetime.
    #[inline]
    pub fn naive_local(&self) -> NaiveDateTime {
        self.datetime + self.offset.fix()
    }
}

/// Convert a `DateTime<Utc>` instance into a `DateTime<FixedOffset>` instance.
impl From<DateTime<Utc>> for DateTime<FixedOffset> {
    /// Convert this `DateTime<Utc>` instance into a `DateTime<FixedOffset>` instance.
    ///
    /// Conversion is done via [`DateTime::with_timezone`]. Note that the converted value returned by
    /// this will be created with a fixed timezone offset of 0.
    fn from(src: DateTime<Utc>) -> Self {
        src.with_timezone(&FixedOffset::east(0))
    }
}

/// Convert a `DateTime<Utc>` instance into a `DateTime<Local>` instance.
#[cfg(feature = "clock")]
impl From<DateTime<Utc>> for DateTime<Local> {
    /// Convert this `DateTime<Utc>` instance into a `DateTime<Local>` instance.
    ///
    /// Conversion is performed via [`DateTime::with_timezone`], accounting for the difference in timezones.
    fn from(src: DateTime<Utc>) -> Self {
        src.with_timezone(&Local)
    }
}

/// Convert a `DateTime<FixedOffset>` instance into a `DateTime<Utc>` instance.
impl From<DateTime<FixedOffset>> for DateTime<Utc> {
    /// Convert this `DateTime<FixedOffset>` instance into a `DateTime<Utc>` instance.
    ///
    /// Conversion is performed via [`DateTime::with_timezone`], accounting for the timezone
    /// difference.
    fn from(src: DateTime<FixedOffset>) -> Self {
        src.with_timezone(&Utc)
    }
}

/// Convert a `DateTime<FixedOffset>` instance into a `DateTime<Local>` instance.
#[cfg(feature = "clock")]
impl From<DateTime<FixedOffset>> for DateTime<Local> {
    /// Convert this `DateTime<FixedOffset>` instance into a `DateTime<Local>` instance.
    ///
    /// Conversion is performed via [`DateTime::with_timezone`]. Returns the equivalent value in local
    /// time.
    fn from(src: DateTime<FixedOffset>) -> Self {
        src.with_timezone(&Local)
    }
}

/// Convert a `DateTime<Local>` instance into a `DateTime<Utc>` instance.
#[cfg(feature = "clock")]
impl From<DateTime<Local>> for DateTime<Utc> {
    /// Convert this `DateTime<Local>` instance into a `DateTime<Utc>` instance.
    ///
    /// Conversion is performed via [`DateTime::with_timezone`], accounting for the difference in
    /// timezones.
    fn from(src: DateTime<Local>) -> Self {
        src.with_timezone(&Utc)
    }
}

/// Convert a `DateTime<Local>` instance into a `DateTime<FixedOffset>` instance.
#[cfg(feature = "clock")]
impl From<DateTime<Local>> for DateTime<FixedOffset> {
    /// Convert this `DateTime<Local>` instance into a `DateTime<FixedOffset>` instance.
    ///
    /// Conversion is performed via [`DateTime::with_timezone`]. Note that the converted value returned
    /// by this will be created with a fixed timezone offset of 0.
    fn from(src: DateTime<Local>) -> Self {
        src.with_timezone(&FixedOffset::east(0))
    }
}

/// Maps the local datetime to other datetime with given conversion function.
fn map_local<Tz: TimeZone, F>(dt: &DateTime<Tz>, mut f: F) -> Option<DateTime<Tz>>
where
    F: FnMut(NaiveDateTime) -> Option<NaiveDateTime>,
{
    f(dt.naive_local()).and_then(|datetime| dt.timezone().from_local_datetime(&datetime).single())
}

impl DateTime<FixedOffset> {
    /// Parses an RFC 2822 date and time string such as `Tue, 1 Jul 2003 10:52:37 +0200`,
    /// then returns a new `DateTime` with a parsed `FixedOffset`.
    ///
    /// RFC 2822 is the internet message standard that specifices the
    /// representation of times in HTTP and email headers.
    ///
    /// ```
    /// # use chrono::{DateTime, FixedOffset, TimeZone};
    /// assert_eq!(
    ///     DateTime::parse_from_rfc2822("Wed, 18 Feb 2015 23:16:09 GMT").unwrap(),
    ///     FixedOffset::east(0).ymd(2015, 2, 18).and_hms(23, 16, 9)
    /// );
    /// ```
    pub fn parse_from_rfc2822(s: &str) -> ParseResult<DateTime<FixedOffset>> {
        const ITEMS: &'static [Item<'static>] = &[Item::Fixed(Fixed::RFC2822)];
        let mut parsed = Parsed::new();
        parse(&mut parsed, s, ITEMS.iter())?;
        parsed.to_datetime()
    }

    /// Parses an RFC 3339 and ISO 8601 date and time string such as `1996-12-19T16:39:57-08:00`,
    /// then returns a new `DateTime` with a parsed `FixedOffset`.
    ///
    /// Why isn't this named `parse_from_iso8601`? That's because ISO 8601 allows some freedom
    /// over the syntax and RFC 3339 exercises that freedom to rigidly define a fixed format.
    pub fn parse_from_rfc3339(s: &str) -> ParseResult<DateTime<FixedOffset>> {
        const ITEMS: &'static [Item<'static>] = &[Item::Fixed(Fixed::RFC3339)];
        let mut parsed = Parsed::new();
        parse(&mut parsed, s, ITEMS.iter())?;
        parsed.to_datetime()
    }

    /// Parses a string with the specified format string and
    /// returns a new `DateTime` with a parsed `FixedOffset`.
    /// See the [`format::strftime` module](./format/strftime/index.html)
    /// on the supported escape sequences.
    ///
    /// See also `Offset::datetime_from_str` which gives a local `DateTime` on specific time zone.
    ///
    /// Note that this method *requires a timezone* in the string. See
    /// [`NaiveDateTime::parse_from_str`](./naive/struct.NaiveDateTime.html#method.parse_from_str)
    /// for a version that does not require a timezone in the to-be-parsed str.
    ///
    /// # Example
    ///
    /// ```rust
    /// use chrono::{DateTime, FixedOffset, TimeZone};
    ///
    /// let dt = DateTime::parse_from_str(
    ///     "1983 Apr 13 12:09:14.274 +0000", "%Y %b %d %H:%M:%S%.3f %z");
    /// assert_eq!(dt, Ok(FixedOffset::east(0).ymd(1983, 4, 13).and_hms_milli(12, 9, 14, 274)));
    /// ```
    pub fn parse_from_str(s: &str, fmt: &str) -> ParseResult<DateTime<FixedOffset>> {
        let mut parsed = Parsed::new();
        parse(&mut parsed, s, StrftimeItems::new(fmt))?;
        parsed.to_datetime()
    }
}

impl<Tz: TimeZone> DateTime<Tz>
where
    Tz::Offset: fmt::Display,
{
    /// Returns an RFC 2822 date and time string such as `Tue, 1 Jul 2003 10:52:37 +0200`.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    pub fn to_rfc2822(&self) -> String {
        const ITEMS: &'static [Item<'static>] = &[Item::Fixed(Fixed::RFC2822)];
        self.format_with_items(ITEMS.iter()).to_string()
    }

    /// Returns an RFC 3339 and ISO 8601 date and time string such as `1996-12-19T16:39:57-08:00`.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    pub fn to_rfc3339(&self) -> String {
        const ITEMS: &'static [Item<'static>] = &[Item::Fixed(Fixed::RFC3339)];
        self.format_with_items(ITEMS.iter()).to_string()
    }

    /// Return an RFC 3339 and ISO 8601 date and time string with subseconds
    /// formatted as per a `SecondsFormat`. If passed `use_z` true and the
    /// timezone is UTC (offset 0), use 'Z', as per
    /// [Fixed::TimezoneOffsetColonZ](format/enum.Fixed.html#variant.TimezoneOffsetColonZ).
    /// If passed `use_z` false, use
    /// [Fixed::TimezoneOffsetColon](format/enum.Fixed.html#variant.TimezoneOffsetColon).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use chrono::{DateTime, FixedOffset, SecondsFormat, TimeZone, Utc};
    /// let dt = Utc.ymd(2018, 1, 26).and_hms_micro(18, 30, 9, 453_829);
    /// assert_eq!(dt.to_rfc3339_opts(SecondsFormat::Millis, false),
    ///            "2018-01-26T18:30:09.453+00:00");
    /// assert_eq!(dt.to_rfc3339_opts(SecondsFormat::Millis, true),
    ///            "2018-01-26T18:30:09.453Z");
    /// assert_eq!(dt.to_rfc3339_opts(SecondsFormat::Secs, true),
    ///            "2018-01-26T18:30:09Z");
    ///
    /// let pst = FixedOffset::east(8 * 60 * 60);
    /// let dt = pst.ymd(2018, 1, 26).and_hms_micro(10, 30, 9, 453_829);
    /// assert_eq!(dt.to_rfc3339_opts(SecondsFormat::Secs, true),
    ///            "2018-01-26T10:30:09+08:00");
    /// ```
    #[cfg(any(feature = "alloc", feature = "std", test))]
    pub fn to_rfc3339_opts(&self, secform: SecondsFormat, use_z: bool) -> String {
        use format::Numeric::*;
        use format::Pad::Zero;
        use SecondsFormat::*;

        debug_assert!(secform != __NonExhaustive, "Do not use __NonExhaustive!");

        const PREFIX: &'static [Item<'static>] = &[
            Item::Numeric(Year, Zero),
            Item::Literal("-"),
            Item::Numeric(Month, Zero),
            Item::Literal("-"),
            Item::Numeric(Day, Zero),
            Item::Literal("T"),
            Item::Numeric(Hour, Zero),
            Item::Literal(":"),
            Item::Numeric(Minute, Zero),
            Item::Literal(":"),
            Item::Numeric(Second, Zero),
        ];

        let ssitem = match secform {
            Secs => None,
            Millis => Some(Item::Fixed(Fixed::Nanosecond3)),
            Micros => Some(Item::Fixed(Fixed::Nanosecond6)),
            Nanos => Some(Item::Fixed(Fixed::Nanosecond9)),
            AutoSi => Some(Item::Fixed(Fixed::Nanosecond)),
            __NonExhaustive => unreachable!(),
        };

        let tzitem = Item::Fixed(if use_z {
            Fixed::TimezoneOffsetColonZ
        } else {
            Fixed::TimezoneOffsetColon
        });

        match ssitem {
            None => self.format_with_items(PREFIX.iter().chain([tzitem].iter())).to_string(),
            Some(s) => self.format_with_items(PREFIX.iter().chain([s, tzitem].iter())).to_string(),
        }
    }

    /// Formats the combined date and time with the specified formatting items.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format_with_items<'a, I, B>(&self, items: I) -> DelayedFormat<I>
    where
        I: Iterator<Item = B> + Clone,
        B: Borrow<Item<'a>>,
    {
        let local = self.naive_local();
        DelayedFormat::new_with_offset(Some(local.date()), Some(local.time()), &self.offset, items)
    }

    /// Formats the combined date and time with the specified format string.
    /// See the [`format::strftime` module](./format/strftime/index.html)
    /// on the supported escape sequences.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.format_with_items(StrftimeItems::new(fmt))
    }

    /// Formats the combined date and time with the specified formatting items and locale.
    #[cfg(feature = "unstable-locales")]
    #[inline]
    pub fn format_localized_with_items<'a, I, B>(
        &self,
        items: I,
        locale: Locale,
    ) -> DelayedFormat<I>
    where
        I: Iterator<Item = B> + Clone,
        B: Borrow<Item<'a>>,
    {
        let local = self.naive_local();
        DelayedFormat::new_with_offset_and_locale(
            Some(local.date()),
            Some(local.time()),
            &self.offset,
            items,
            locale,
        )
    }

    /// Formats the combined date and time with the specified format string and locale.
    /// See the [`format::strftime` module](./format/strftime/index.html)
    /// on the supported escape sequences.
    #[cfg(feature = "unstable-locales")]
    #[inline]
    pub fn format_localized<'a>(
        &self,
        fmt: &'a str,
        locale: Locale,
    ) -> DelayedFormat<StrftimeItems<'a>> {
        self.format_localized_with_items(StrftimeItems::new_with_locale(fmt, locale), locale)
    }
}

impl<Tz: TimeZone> Datelike for DateTime<Tz> {
    #[inline]
    fn year(&self) -> i32 {
        self.naive_local().year()
    }
    #[inline]
    fn month(&self) -> u32 {
        self.naive_local().month()
    }
    #[inline]
    fn month0(&self) -> u32 {
        self.naive_local().month0()
    }
    #[inline]
    fn day(&self) -> u32 {
        self.naive_local().day()
    }
    #[inline]
    fn day0(&self) -> u32 {
        self.naive_local().day0()
    }
    #[inline]
    fn ordinal(&self) -> u32 {
        self.naive_local().ordinal()
    }
    #[inline]
    fn ordinal0(&self) -> u32 {
        self.naive_local().ordinal0()
    }
    #[inline]
    fn weekday(&self) -> Weekday {
        self.naive_local().weekday()
    }
    #[inline]
    fn iso_week(&self) -> IsoWeek {
        self.naive_local().iso_week()
    }

    #[inline]
    fn with_year(&self, year: i32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_year(year))
    }

    #[inline]
    fn with_month(&self, month: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_month(month))
    }

    #[inline]
    fn with_month0(&self, month0: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_month0(month0))
    }

    #[inline]
    fn with_day(&self, day: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_day(day))
    }

    #[inline]
    fn with_day0(&self, day0: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_day0(day0))
    }

    #[inline]
    fn with_ordinal(&self, ordinal: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_ordinal(ordinal))
    }

    #[inline]
    fn with_ordinal0(&self, ordinal0: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_ordinal0(ordinal0))
    }
}

impl<Tz: TimeZone> Timelike for DateTime<Tz> {
    #[inline]
    fn hour(&self) -> u32 {
        self.naive_local().hour()
    }
    #[inline]
    fn minute(&self) -> u32 {
        self.naive_local().minute()
    }
    #[inline]
    fn second(&self) -> u32 {
        self.naive_local().second()
    }
    #[inline]
    fn nanosecond(&self) -> u32 {
        self.naive_local().nanosecond()
    }

    #[inline]
    fn with_hour(&self, hour: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_hour(hour))
    }

    #[inline]
    fn with_minute(&self, min: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_minute(min))
    }

    #[inline]
    fn with_second(&self, sec: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_second(sec))
    }

    #[inline]
    fn with_nanosecond(&self, nano: u32) -> Option<DateTime<Tz>> {
        map_local(self, |datetime| datetime.with_nanosecond(nano))
    }
}

// we need them as automatic impls cannot handle associated types
impl<Tz: TimeZone> Copy for DateTime<Tz> where <Tz as TimeZone>::Offset: Copy {}
unsafe impl<Tz: TimeZone> Send for DateTime<Tz> where <Tz as TimeZone>::Offset: Send {}

impl<Tz: TimeZone, Tz2: TimeZone> PartialEq<DateTime<Tz2>> for DateTime<Tz> {
    fn eq(&self, other: &DateTime<Tz2>) -> bool {
        self.datetime == other.datetime
    }
}

impl<Tz: TimeZone> Eq for DateTime<Tz> {}

impl<Tz: TimeZone, Tz2: TimeZone> PartialOrd<DateTime<Tz2>> for DateTime<Tz> {
    /// Compare two DateTimes based on their true time, ignoring time zones
    ///
    /// # Example
    ///
    /// ```
    /// use chrono::prelude::*;
    ///
    /// let earlier = Utc.ymd(2015, 5, 15).and_hms(2, 0, 0).with_timezone(&FixedOffset::west(1 * 3600));
    /// let later   = Utc.ymd(2015, 5, 15).and_hms(3, 0, 0).with_timezone(&FixedOffset::west(5 * 3600));
    ///
    /// assert_eq!(earlier.to_string(), "2015-05-15 01:00:00 -01:00");
    /// assert_eq!(later.to_string(), "2015-05-14 22:00:00 -05:00");
    ///
    /// assert!(later > earlier);
    /// ```
    fn partial_cmp(&self, other: &DateTime<Tz2>) -> Option<Ordering> {
        self.datetime.partial_cmp(&other.datetime)
    }
}

impl<Tz: TimeZone> Ord for DateTime<Tz> {
    fn cmp(&self, other: &DateTime<Tz>) -> Ordering {
        self.datetime.cmp(&other.datetime)
    }
}

impl<Tz: TimeZone> hash::Hash for DateTime<Tz> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.datetime.hash(state)
    }
}

impl<Tz: TimeZone> Add<OldDuration> for DateTime<Tz> {
    type Output = DateTime<Tz>;

    #[inline]
    fn add(self, rhs: OldDuration) -> DateTime<Tz> {
        self.checked_add_signed(rhs).expect("`DateTime + Duration` overflowed")
    }
}

impl<Tz: TimeZone> Sub<OldDuration> for DateTime<Tz> {
    type Output = DateTime<Tz>;

    #[inline]
    fn sub(self, rhs: OldDuration) -> DateTime<Tz> {
        self.checked_sub_signed(rhs).expect("`DateTime - Duration` overflowed")
    }
}

impl<Tz: TimeZone> Sub<DateTime<Tz>> for DateTime<Tz> {
    type Output = OldDuration;

    #[inline]
    fn sub(self, rhs: DateTime<Tz>) -> OldDuration {
        self.signed_duration_since(rhs)
    }
}

impl<Tz: TimeZone> fmt::Debug for DateTime<Tz> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}{:?}", self.naive_local(), self.offset)
    }
}

impl<Tz: TimeZone> fmt::Display for DateTime<Tz>
where
    Tz::Offset: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.naive_local(), self.offset)
    }
}

impl str::FromStr for DateTime<Utc> {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<DateTime<Utc>> {
        s.parse::<DateTime<FixedOffset>>().map(|dt| dt.with_timezone(&Utc))
    }
}

#[cfg(feature = "clock")]
impl str::FromStr for DateTime<Local> {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<DateTime<Local>> {
        s.parse::<DateTime<FixedOffset>>().map(|dt| dt.with_timezone(&Local))
    }
}

#[cfg(any(feature = "std", test))]
impl From<SystemTime> for DateTime<Utc> {
    fn from(t: SystemTime) -> DateTime<Utc> {
        let (sec, nsec) = match t.duration_since(UNIX_EPOCH) {
            Ok(dur) => (dur.as_secs() as i64, dur.subsec_nanos()),
            Err(e) => {
                // unlikely but should be handled
                let dur = e.duration();
                let (sec, nsec) = (dur.as_secs() as i64, dur.subsec_nanos());
                if nsec == 0 {
                    (-sec, 0)
                } else {
                    (-sec - 1, 1_000_000_000 - nsec)
                }
            }
        };
        Utc.timestamp(sec, nsec)
    }
}

#[cfg(feature = "clock")]
impl From<SystemTime> for DateTime<Local> {
    fn from(t: SystemTime) -> DateTime<Local> {
        DateTime::<Utc>::from(t).with_timezone(&Local)
    }
}

#[cfg(any(feature = "std", test))]
impl<Tz: TimeZone> From<DateTime<Tz>> for SystemTime {
    fn from(dt: DateTime<Tz>) -> SystemTime {
        use std::time::Duration;

        let sec = dt.timestamp();
        let nsec = dt.timestamp_subsec_nanos();
        if sec < 0 {
            // unlikely but should be handled
            UNIX_EPOCH - Duration::new(-sec as u64, 0) + Duration::new(0, nsec)
        } else {
            UNIX_EPOCH + Duration::new(sec as u64, nsec)
        }
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
impl From<js_sys::Date> for DateTime<Utc> {
    fn from(date: js_sys::Date) -> DateTime<Utc> {
        DateTime::<Utc>::from(&date)
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
impl From<&js_sys::Date> for DateTime<Utc> {
    fn from(date: &js_sys::Date) -> DateTime<Utc> {
        let millisecs_since_unix_epoch: u64 = date.get_time() as u64;
        let secs = millisecs_since_unix_epoch / 1000;
        let nanos = 1_000_000 * (millisecs_since_unix_epoch % 1000);
        let naive = NaiveDateTime::from_timestamp(secs as i64, nanos as u32);
        DateTime::from_utc(naive, Utc)
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
impl From<DateTime<Utc>> for js_sys::Date {
    fn from(date: DateTime<Utc>) -> js_sys::Date {
        let js_date = js_sys::Date::new_0();

        js_date.set_utc_full_year_with_month_date(
            date.year() as u32,
            date.month0() as i32,
            date.day() as i32,
        );

        js_date.set_utc_hours(date.hour());
        js_date.set_utc_minutes(date.minute());
        js_date.set_utc_seconds(date.second());

        js_date
    }
}

#[test]
fn test_auto_conversion() {
    let utc_dt = Utc.ymd(2018, 9, 5).and_hms(23, 58, 0);
    let cdt_dt = FixedOffset::west(5 * 60 * 60).ymd(2018, 9, 5).and_hms(18, 58, 0);
    let utc_dt2: DateTime<Utc> = cdt_dt.into();
    assert_eq!(utc_dt, utc_dt2);
}

#[cfg(all(test, any(feature = "rustc-serialize", feature = "serde")))]
fn test_encodable_json<FUtc, FFixed, E>(to_string_utc: FUtc, to_string_fixed: FFixed)
where
    FUtc: Fn(&DateTime<Utc>) -> Result<String, E>,
    FFixed: Fn(&DateTime<FixedOffset>) -> Result<String, E>,
    E: ::core::fmt::Debug,
{
    assert_eq!(
        to_string_utc(&Utc.ymd(2014, 7, 24).and_hms(12, 34, 6)).ok(),
        Some(r#""2014-07-24T12:34:06Z""#.into())
    );

    assert_eq!(
        to_string_fixed(&FixedOffset::east(3660).ymd(2014, 7, 24).and_hms(12, 34, 6)).ok(),
        Some(r#""2014-07-24T12:34:06+01:01""#.into())
    );
    assert_eq!(
        to_string_fixed(&FixedOffset::east(3650).ymd(2014, 7, 24).and_hms(12, 34, 6)).ok(),
        Some(r#""2014-07-24T12:34:06+01:00:50""#.into())
    );
}

#[cfg(all(test, feature = "clock", any(feature = "rustc-serialize", feature = "serde")))]
fn test_decodable_json<FUtc, FFixed, FLocal, E>(
    utc_from_str: FUtc,
    fixed_from_str: FFixed,
    local_from_str: FLocal,
) where
    FUtc: Fn(&str) -> Result<DateTime<Utc>, E>,
    FFixed: Fn(&str) -> Result<DateTime<FixedOffset>, E>,
    FLocal: Fn(&str) -> Result<DateTime<Local>, E>,
    E: ::core::fmt::Debug,
{
    // should check against the offset as well (the normal DateTime comparison will ignore them)
    fn norm<Tz: TimeZone>(dt: &Option<DateTime<Tz>>) -> Option<(&DateTime<Tz>, &Tz::Offset)> {
        dt.as_ref().map(|dt| (dt, dt.offset()))
    }

    assert_eq!(
        norm(&utc_from_str(r#""2014-07-24T12:34:06Z""#).ok()),
        norm(&Some(Utc.ymd(2014, 7, 24).and_hms(12, 34, 6)))
    );
    assert_eq!(
        norm(&utc_from_str(r#""2014-07-24T13:57:06+01:23""#).ok()),
        norm(&Some(Utc.ymd(2014, 7, 24).and_hms(12, 34, 6)))
    );

    assert_eq!(
        norm(&fixed_from_str(r#""2014-07-24T12:34:06Z""#).ok()),
        norm(&Some(FixedOffset::east(0).ymd(2014, 7, 24).and_hms(12, 34, 6)))
    );
    assert_eq!(
        norm(&fixed_from_str(r#""2014-07-24T13:57:06+01:23""#).ok()),
        norm(&Some(FixedOffset::east(60 * 60 + 23 * 60).ymd(2014, 7, 24).and_hms(13, 57, 6)))
    );

    // we don't know the exact local offset but we can check that
    // the conversion didn't change the instant itself
    assert_eq!(
        local_from_str(r#""2014-07-24T12:34:06Z""#).expect("local shouuld parse"),
        Utc.ymd(2014, 7, 24).and_hms(12, 34, 6)
    );
    assert_eq!(
        local_from_str(r#""2014-07-24T13:57:06+01:23""#).expect("local should parse with offset"),
        Utc.ymd(2014, 7, 24).and_hms(12, 34, 6)
    );

    assert!(utc_from_str(r#""2014-07-32T12:34:06Z""#).is_err());
    assert!(fixed_from_str(r#""2014-07-32T12:34:06Z""#).is_err());
}

#[cfg(all(test, feature = "clock", feature = "rustc-serialize"))]
fn test_decodable_json_timestamps<FUtc, FFixed, FLocal, E>(
    utc_from_str: FUtc,
    fixed_from_str: FFixed,
    local_from_str: FLocal,
) where
    FUtc: Fn(&str) -> Result<rustc_serialize::TsSeconds<Utc>, E>,
    FFixed: Fn(&str) -> Result<rustc_serialize::TsSeconds<FixedOffset>, E>,
    FLocal: Fn(&str) -> Result<rustc_serialize::TsSeconds<Local>, E>,
    E: ::core::fmt::Debug,
{
    fn norm<Tz: TimeZone>(dt: &Option<DateTime<Tz>>) -> Option<(&DateTime<Tz>, &Tz::Offset)> {
        dt.as_ref().map(|dt| (dt, dt.offset()))
    }

    assert_eq!(
        norm(&utc_from_str("0").ok().map(DateTime::from)),
        norm(&Some(Utc.ymd(1970, 1, 1).and_hms(0, 0, 0)))
    );
    assert_eq!(
        norm(&utc_from_str("-1").ok().map(DateTime::from)),
        norm(&Some(Utc.ymd(1969, 12, 31).and_hms(23, 59, 59)))
    );

    assert_eq!(
        norm(&fixed_from_str("0").ok().map(DateTime::from)),
        norm(&Some(FixedOffset::east(0).ymd(1970, 1, 1).and_hms(0, 0, 0)))
    );
    assert_eq!(
        norm(&fixed_from_str("-1").ok().map(DateTime::from)),
        norm(&Some(FixedOffset::east(0).ymd(1969, 12, 31).and_hms(23, 59, 59)))
    );

    assert_eq!(
        *fixed_from_str("0").expect("0 timestamp should parse"),
        Utc.ymd(1970, 1, 1).and_hms(0, 0, 0)
    );
    assert_eq!(
        *local_from_str("-1").expect("-1 timestamp should parse"),
        Utc.ymd(1969, 12, 31).and_hms(23, 59, 59)
    );
}

#[cfg(feature = "rustc-serialize")]
pub mod rustc_serialize {
    use super::DateTime;
    use core::fmt;
    use core::ops::Deref;
    #[cfg(feature = "clock")]
    use offset::Local;
    use offset::{FixedOffset, LocalResult, TimeZone, Utc};
    use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};

    impl<Tz: TimeZone> Encodable for DateTime<Tz> {
        fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
            format!("{:?}", self).encode(s)
        }
    }

    // lik? function to convert a LocalResult into a serde-ish Result
    fn from<T, D>(me: LocalResult<T>, d: &mut D) -> Result<T, D::Error>
    where
        D: Decoder,
        T: fmt::Display,
    {
        match me {
            LocalResult::None => Err(d.error("value is not a legal timestamp")),
            LocalResult::Ambiguous(..) => Err(d.error("value is an ambiguous timestamp")),
            LocalResult::Single(val) => Ok(val),
        }
    }

    impl Decodable for DateTime<FixedOffset> {
        fn decode<D: Decoder>(d: &mut D) -> Result<DateTime<FixedOffset>, D::Error> {
            d.read_str()?
                .parse::<DateTime<FixedOffset>>()
                .map_err(|_| d.error("invalid date and time"))
        }
    }

    #[allow(deprecated)]
    impl Decodable for TsSeconds<FixedOffset> {
        #[allow(deprecated)]
        fn decode<D: Decoder>(d: &mut D) -> Result<TsSeconds<FixedOffset>, D::Error> {
            from(FixedOffset::east(0).timestamp_opt(d.read_i64()?, 0), d).map(TsSeconds)
        }
    }

    impl Decodable for DateTime<Utc> {
        fn decode<D: Decoder>(d: &mut D) -> Result<DateTime<Utc>, D::Error> {
            d.read_str()?
                .parse::<DateTime<FixedOffset>>()
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| d.error("invalid date and time"))
        }
    }

    /// A `DateTime` that can be deserialized from a timestamp
    ///
    /// A timestamp here is seconds since the epoch
    #[derive(Debug)]
    pub struct TsSeconds<Tz: TimeZone>(DateTime<Tz>);

    #[allow(deprecated)]
    impl<Tz: TimeZone> From<TsSeconds<Tz>> for DateTime<Tz> {
        /// Pull the inner DateTime<Tz> out
        #[allow(deprecated)]
        fn from(obj: TsSeconds<Tz>) -> DateTime<Tz> {
            obj.0
        }
    }

    #[allow(deprecated)]
    impl<Tz: TimeZone> Deref for TsSeconds<Tz> {
        type Target = DateTime<Tz>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    #[allow(deprecated)]
    impl Decodable for TsSeconds<Utc> {
        fn decode<D: Decoder>(d: &mut D) -> Result<TsSeconds<Utc>, D::Error> {
            from(Utc.timestamp_opt(d.read_i64()?, 0), d).map(TsSeconds)
        }
    }

    #[cfg(feature = "clock")]
    impl Decodable for DateTime<Local> {
        fn decode<D: Decoder>(d: &mut D) -> Result<DateTime<Local>, D::Error> {
            match d.read_str()?.parse::<DateTime<FixedOffset>>() {
                Ok(dt) => Ok(dt.with_timezone(&Local)),
                Err(_) => Err(d.error("invalid date and time")),
            }
        }
    }

    #[cfg(feature = "clock")]
    #[allow(deprecated)]
    impl Decodable for TsSeconds<Local> {
        #[allow(deprecated)]
        fn decode<D: Decoder>(d: &mut D) -> Result<TsSeconds<Local>, D::Error> {
            from(Utc.timestamp_opt(d.read_i64()?, 0), d)
                .map(|dt| TsSeconds(dt.with_timezone(&Local)))
        }
    }

    #[cfg(test)]
    use rustc_serialize::json;

    #[test]
    fn test_encodable() {
        super::test_encodable_json(json::encode, json::encode);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_decodable() {
        super::test_decodable_json(json::decode, json::decode, json::decode);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_decodable_timestamps() {
        super::test_decodable_json_timestamps(json::decode, json::decode, json::decode);
    }
}

/// documented at re-export site
#[cfg(feature = "serde")]
pub mod serde {
    use super::DateTime;
    use core::fmt;
    #[cfg(feature = "clock")]
    use offset::Local;
    use offset::{FixedOffset, LocalResult, TimeZone, Utc};
    use serdelib::{de, ser};
    use {ne_timestamp, SerdeError};

    #[doc(hidden)]
    #[derive(Debug)]
    pub struct SecondsTimestampVisitor;

    #[doc(hidden)]
    #[derive(Debug)]
    pub struct NanoSecondsTimestampVisitor;

    #[doc(hidden)]
    #[derive(Debug)]
    pub struct MilliSecondsTimestampVisitor;

    // lik? function to convert a LocalResult into a serde-ish Result
    fn serde_from<T, E, V>(me: LocalResult<T>, ts: &V) -> Result<T, E>
    where
        E: de::Error,
        V: fmt::Display,
        T: fmt::Display,
    {
        match me {
            LocalResult::None => Err(E::custom(ne_timestamp(ts))),
            LocalResult::Ambiguous(min, max) => {
                Err(E::custom(SerdeError::Ambiguous { timestamp: ts, min: min, max: max }))
            }
            LocalResult::Single(val) => Ok(val),
        }
    }

    /// Ser/de to/from timestamps in nanoseconds
    ///
    /// Intended for use with `serde`'s `with` attribute.
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
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_nanoseconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_nanoseconds")]
    ///     time: DateTime<Utc>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Utc.ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733);
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

        use offset::TimeZone;
        use {DateTime, Utc};

        use super::{serde_from, NanoSecondsTimestampVisitor};

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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_nanoseconds::serialize as to_nano_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_nano_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Utc.ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918355733}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.serialize_i64(dt.timestamp_nanos())
        }

        /// Deserialize a `DateTime` from a nanosecond timestamp
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
        /// # extern crate chrono;
        /// # use chrono::{DateTime, Utc};
        /// use chrono::serde::ts_nanoseconds::deserialize as from_nano_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_nano_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1526522699918355733 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Utc>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(NanoSecondsTimestampVisitor)?)
        }

        impl<'de> de::Visitor<'de> for NanoSecondsTimestampVisitor {
            type Value = DateTime<Utc>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a unix timestamp in nanoseconds")
            }

            /// Deserialize a timestamp in nanoseconds since the epoch
            fn visit_i64<E>(self, value: i64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(
                    Utc.timestamp_opt(value / 1_000_000_000, (value % 1_000_000_000) as u32),
                    &value,
                )
            }

            /// Deserialize a timestamp in nanoseconds since the epoch
            fn visit_u64<E>(self, value: u64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(
                    Utc.timestamp_opt(
                        (value / 1_000_000_000) as i64,
                        (value % 1_000_000_000) as u32,
                    ),
                    &value,
                )
            }
        }
    }

    /// Ser/de to/from optional timestamps in nanoseconds
    ///
    /// Intended for use with `serde`'s `with` attribute.
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
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_nanoseconds_option;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_nanoseconds_option")]
    ///     time: Option<DateTime<Utc>>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Some(Utc.ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733));
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
    pub mod ts_nanoseconds_option {
        use core::fmt;
        use serdelib::{de, ser};

        use {DateTime, Utc};

        use super::NanoSecondsTimestampVisitor;

        /// Serialize a UTC datetime into an integer number of nanoseconds since the epoch or none
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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_nanoseconds_option::serialize as to_nano_tsopt;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_nano_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Some(Utc.ymd(2018, 5, 17).and_hms_nano(02, 04, 59, 918355733)),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918355733}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(opt: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            match *opt {
                Some(ref dt) => serializer.serialize_some(&dt.timestamp_nanos()),
                None => serializer.serialize_none(),
            }
        }

        /// Deserialize a `DateTime` from a nanosecond timestamp or none
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
        /// # extern crate chrono;
        /// # use chrono::{DateTime, Utc};
        /// use chrono::serde::ts_nanoseconds_option::deserialize as from_nano_tsopt;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_nano_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1526522699918355733 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<Option<DateTime<Utc>>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_option(OptionNanoSecondsTimestampVisitor)?)
        }

        struct OptionNanoSecondsTimestampVisitor;

        impl<'de> de::Visitor<'de> for OptionNanoSecondsTimestampVisitor {
            type Value = Option<DateTime<Utc>>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp in nanoseconds or none")
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_some<D>(self, d: D) -> Result<Option<DateTime<Utc>>, D::Error>
            where
                D: de::Deserializer<'de>,
            {
                d.deserialize_i64(NanoSecondsTimestampVisitor).map(Some)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_none<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_unit<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }
    }

    /// Ser/de to/from timestamps in milliseconds
    ///
    /// Intended for use with `serde`s `with` attribute.
    ///
    /// # Example
    ///
    /// ```rust
    /// # // We mark this ignored so that we can test on 1.13 (which does not
    /// # // support custom derive), and run tests with --ignored on beta and
    /// # // nightly to actually trigger these.
    /// #
    /// # #[macro_use] extern crate serde_derive;
    /// # #[macro_use] extern crate serde_json;
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_milliseconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_milliseconds")]
    ///     time: DateTime<Utc>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Utc.ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918);
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

        use offset::TimeZone;
        use {DateTime, Utc};

        use super::{serde_from, MilliSecondsTimestampVisitor};

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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_milliseconds::serialize as to_milli_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_milli_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Utc.ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            serializer.serialize_i64(dt.timestamp_millis())
        }

        /// Deserialize a `DateTime` from a millisecond timestamp
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
        /// # extern crate chrono;
        /// # use chrono::{DateTime, Utc};
        /// use chrono::serde::ts_milliseconds::deserialize as from_milli_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_milli_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1526522699918 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Utc>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(MilliSecondsTimestampVisitor).map(|dt| dt.with_timezone(&Utc))?)
        }

        impl<'de> de::Visitor<'de> for MilliSecondsTimestampVisitor {
            type Value = DateTime<Utc>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp in milliseconds")
            }

            /// Deserialize a timestamp in milliseconds since the epoch
            fn visit_i64<E>(self, value: i64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(
                    Utc.timestamp_opt(value / 1000, ((value % 1000) * 1_000_000) as u32),
                    &value,
                )
            }

            /// Deserialize a timestamp in milliseconds since the epoch
            fn visit_u64<E>(self, value: u64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(
                    Utc.timestamp_opt((value / 1000) as i64, ((value % 1000) * 1_000_000) as u32),
                    &value,
                )
            }
        }
    }

    /// Ser/de to/from optional timestamps in milliseconds
    ///
    /// Intended for use with `serde`s `with` attribute.
    ///
    /// # Example
    ///
    /// ```rust
    /// # // We mark this ignored so that we can test on 1.13 (which does not
    /// # // support custom derive), and run tests with --ignored on beta and
    /// # // nightly to actually trigger these.
    /// #
    /// # #[macro_use] extern crate serde_derive;
    /// # #[macro_use] extern crate serde_json;
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_milliseconds_option;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_milliseconds_option")]
    ///     time: Option<DateTime<Utc>>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Some(Utc.ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918));
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
    pub mod ts_milliseconds_option {
        use core::fmt;
        use serdelib::{de, ser};

        use {DateTime, Utc};

        use super::MilliSecondsTimestampVisitor;

        /// Serialize a UTC datetime into an integer number of milliseconds since the epoch or none
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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_milliseconds_option::serialize as to_milli_tsopt;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_milli_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Some(Utc.ymd(2018, 5, 17).and_hms_milli(02, 04, 59, 918)),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1526522699918}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(opt: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            match *opt {
                Some(ref dt) => serializer.serialize_some(&dt.timestamp_millis()),
                None => serializer.serialize_none(),
            }
        }

        /// Deserialize a `DateTime` from a millisecond timestamp or none
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
        /// # extern crate chrono;
        /// # use chrono::prelude::*;
        /// use chrono::serde::ts_milliseconds_option::deserialize as from_milli_tsopt;
        ///
        /// #[derive(Deserialize, PartialEq, Debug)]
        /// #[serde(untagged)]
        /// enum E<T> {
        ///     V(T),
        /// }
        ///
        /// #[derive(Deserialize, PartialEq, Debug)]
        /// struct S {
        ///     #[serde(default, deserialize_with = "from_milli_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<(), serde_json::Error> {
        /// let my_s: E<S> = serde_json::from_str(r#"{ "time": 1526522699918 }"#)?;
        /// assert_eq!(my_s, E::V(S { time: Some(Utc.timestamp(1526522699, 918000000)) }));
        /// let s: E<S> = serde_json::from_str(r#"{ "time": null }"#)?;
        /// assert_eq!(s, E::V(S { time: None }));
        /// let t: E<S> = serde_json::from_str(r#"{}"#)?;
        /// assert_eq!(t, E::V(S { time: None }));
        /// # Ok(())
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<Option<DateTime<Utc>>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_option(OptionMilliSecondsTimestampVisitor)
                .map(|opt| opt.map(|dt| dt.with_timezone(&Utc)))?)
        }

        struct OptionMilliSecondsTimestampVisitor;

        impl<'de> de::Visitor<'de> for OptionMilliSecondsTimestampVisitor {
            type Value = Option<DateTime<Utc>>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp in milliseconds or none")
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_some<D>(self, d: D) -> Result<Option<DateTime<Utc>>, D::Error>
            where
                D: de::Deserializer<'de>,
            {
                d.deserialize_i64(MilliSecondsTimestampVisitor).map(Some)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_none<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_unit<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }
    }

    /// Ser/de to/from timestamps in seconds
    ///
    /// Intended for use with `serde`'s `with` attribute.
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
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_seconds;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_seconds")]
    ///     time: DateTime<Utc>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Utc.ymd(2015, 5, 15).and_hms(10, 0, 0);
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

        use offset::TimeZone;
        use {DateTime, Utc};

        use super::{serde_from, SecondsTimestampVisitor};

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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_seconds::serialize as to_ts;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Utc.ymd(2015, 5, 15).and_hms(10, 0, 0),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1431684000}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
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
        /// # extern crate chrono;
        /// # use chrono::{DateTime, Utc};
        /// use chrono::serde::ts_seconds::deserialize as from_ts;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_ts")]
        ///     time: DateTime<Utc>
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1431684000 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Utc>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_i64(SecondsTimestampVisitor)?)
        }

        impl<'de> de::Visitor<'de> for SecondsTimestampVisitor {
            type Value = DateTime<Utc>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp in seconds")
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_i64<E>(self, value: i64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(Utc.timestamp_opt(value, 0), &value)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_u64<E>(self, value: u64) -> Result<DateTime<Utc>, E>
            where
                E: de::Error,
            {
                serde_from(Utc.timestamp_opt(value as i64, 0), &value)
            }
        }
    }

    /// Ser/de to/from optional timestamps in seconds
    ///
    /// Intended for use with `serde`'s `with` attribute.
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
    /// # extern crate chrono;
    /// # use chrono::{TimeZone, DateTime, Utc};
    /// use chrono::serde::ts_seconds_option;
    /// #[derive(Deserialize, Serialize)]
    /// struct S {
    ///     #[serde(with = "ts_seconds_option")]
    ///     time: Option<DateTime<Utc>>
    /// }
    ///
    /// # fn example() -> Result<S, serde_json::Error> {
    /// let time = Some(Utc.ymd(2015, 5, 15).and_hms(10, 0, 0));
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
    pub mod ts_seconds_option {
        use core::fmt;
        use serdelib::{de, ser};

        use {DateTime, Utc};

        use super::SecondsTimestampVisitor;

        /// Serialize a UTC datetime into an integer number of seconds since the epoch or none
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
        /// # extern crate chrono;
        /// # use chrono::{TimeZone, DateTime, Utc};
        /// use chrono::serde::ts_seconds_option::serialize as to_tsopt;
        /// #[derive(Serialize)]
        /// struct S {
        ///     #[serde(serialize_with = "to_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<String, serde_json::Error> {
        /// let my_s = S {
        ///     time: Some(Utc.ymd(2015, 5, 15).and_hms(10, 0, 0)),
        /// };
        /// let as_string = serde_json::to_string(&my_s)?;
        /// assert_eq!(as_string, r#"{"time":1431684000}"#);
        /// # Ok(as_string)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn serialize<S>(opt: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            match *opt {
                Some(ref dt) => serializer.serialize_some(&dt.timestamp()),
                None => serializer.serialize_none(),
            }
        }

        /// Deserialize a `DateTime` from a seconds timestamp or none
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
        /// # extern crate chrono;
        /// # use chrono::{DateTime, Utc};
        /// use chrono::serde::ts_seconds_option::deserialize as from_tsopt;
        /// #[derive(Deserialize)]
        /// struct S {
        ///     #[serde(deserialize_with = "from_tsopt")]
        ///     time: Option<DateTime<Utc>>
        /// }
        ///
        /// # fn example() -> Result<S, serde_json::Error> {
        /// let my_s: S = serde_json::from_str(r#"{ "time": 1431684000 }"#)?;
        /// # Ok(my_s)
        /// # }
        /// # fn main() { example().unwrap(); }
        /// ```
        pub fn deserialize<'de, D>(d: D) -> Result<Option<DateTime<Utc>>, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            Ok(d.deserialize_option(OptionSecondsTimestampVisitor)?)
        }

        struct OptionSecondsTimestampVisitor;

        impl<'de> de::Visitor<'de> for OptionSecondsTimestampVisitor {
            type Value = Option<DateTime<Utc>>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unix timestamp in seconds or none")
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_some<D>(self, d: D) -> Result<Option<DateTime<Utc>>, D::Error>
            where
                D: de::Deserializer<'de>,
            {
                d.deserialize_i64(SecondsTimestampVisitor).map(Some)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_none<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            /// Deserialize a timestamp in seconds since the epoch
            fn visit_unit<E>(self) -> Result<Option<DateTime<Utc>>, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }
    }

    impl<Tz: TimeZone> ser::Serialize for DateTime<Tz> {
        /// Serialize into a rfc3339 time string
        ///
        /// See [the `serde` module](./serde/index.html) for alternate
        /// serializations.
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

            // Debug formatting is correct RFC3339, and it allows Zulu.
            serializer.collect_str(&FormatWrapped { inner: &self })
        }
    }

    struct DateTimeVisitor;

    impl<'de> de::Visitor<'de> for DateTimeVisitor {
        type Value = DateTime<FixedOffset>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a formatted date and time string or a unix timestamp")
        }

        fn visit_str<E>(self, value: &str) -> Result<DateTime<FixedOffset>, E>
        where
            E: de::Error,
        {
            value.parse().map_err(|err: ::format::ParseError| E::custom(err))
        }
    }

    /// Deserialize a value that optionally includes a timezone offset in its
    /// string representation
    ///
    /// The value to be deserialized must be an rfc3339 string.
    ///
    /// See [the `serde` module](./serde/index.html) for alternate
    /// deserialization formats.
    impl<'de> de::Deserialize<'de> for DateTime<FixedOffset> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_str(DateTimeVisitor)
        }
    }

    /// Deserialize into a UTC value
    ///
    /// The value to be deserialized must be an rfc3339 string.
    ///
    /// See [the `serde` module](./serde/index.html) for alternate
    /// deserialization formats.
    impl<'de> de::Deserialize<'de> for DateTime<Utc> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_str(DateTimeVisitor).map(|dt| dt.with_timezone(&Utc))
        }
    }

    /// Deserialize a value that includes no timezone in its string
    /// representation
    ///
    /// The value to be deserialized must be an rfc3339 string.
    ///
    /// See [the `serde` module](./serde/index.html) for alternate
    /// serialization formats.
    #[cfg(feature = "clock")]
    impl<'de> de::Deserialize<'de> for DateTime<Local> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_str(DateTimeVisitor).map(|dt| dt.with_timezone(&Local))
        }
    }

    #[cfg(test)]
    extern crate bincode;
    #[cfg(test)]
    extern crate serde_json;

    #[test]
    fn test_serde_serialize() {
        super::test_encodable_json(self::serde_json::to_string, self::serde_json::to_string);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_serde_deserialize() {
        super::test_decodable_json(
            |input| self::serde_json::from_str(&input),
            |input| self::serde_json::from_str(&input),
            |input| self::serde_json::from_str(&input),
        );
    }

    #[test]
    fn test_serde_bincode() {
        // Bincode is relevant to test separately from JSON because
        // it is not self-describing.
        use self::bincode::{deserialize, serialize, Infinite};

        let dt = Utc.ymd(2014, 7, 24).and_hms(12, 34, 6);
        let encoded = serialize(&dt, Infinite).unwrap();
        let decoded: DateTime<Utc> = deserialize(&encoded).unwrap();
        assert_eq!(dt, decoded);
        assert_eq!(dt.offset(), decoded.offset());
    }
}

#[cfg(test)]
mod tests {
    use super::DateTime;
    use naive::{NaiveDate, NaiveTime};
    #[cfg(feature = "clock")]
    use offset::Local;
    use offset::{FixedOffset, TimeZone, Utc};
    use oldtime::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[cfg(feature = "clock")]
    use Datelike;

    #[test]
    #[allow(non_snake_case)]
    fn test_datetime_offset() {
        let Est = FixedOffset::west(5 * 60 * 60);
        let Edt = FixedOffset::west(4 * 60 * 60);
        let Kst = FixedOffset::east(9 * 60 * 60);

        assert_eq!(format!("{}", Utc.ymd(2014, 5, 6).and_hms(7, 8, 9)), "2014-05-06 07:08:09 UTC");
        assert_eq!(
            format!("{}", Edt.ymd(2014, 5, 6).and_hms(7, 8, 9)),
            "2014-05-06 07:08:09 -04:00"
        );
        assert_eq!(
            format!("{}", Kst.ymd(2014, 5, 6).and_hms(7, 8, 9)),
            "2014-05-06 07:08:09 +09:00"
        );
        assert_eq!(format!("{:?}", Utc.ymd(2014, 5, 6).and_hms(7, 8, 9)), "2014-05-06T07:08:09Z");
        assert_eq!(
            format!("{:?}", Edt.ymd(2014, 5, 6).and_hms(7, 8, 9)),
            "2014-05-06T07:08:09-04:00"
        );
        assert_eq!(
            format!("{:?}", Kst.ymd(2014, 5, 6).and_hms(7, 8, 9)),
            "2014-05-06T07:08:09+09:00"
        );

        // edge cases
        assert_eq!(format!("{:?}", Utc.ymd(2014, 5, 6).and_hms(0, 0, 0)), "2014-05-06T00:00:00Z");
        assert_eq!(
            format!("{:?}", Edt.ymd(2014, 5, 6).and_hms(0, 0, 0)),
            "2014-05-06T00:00:00-04:00"
        );
        assert_eq!(
            format!("{:?}", Kst.ymd(2014, 5, 6).and_hms(0, 0, 0)),
            "2014-05-06T00:00:00+09:00"
        );
        assert_eq!(
            format!("{:?}", Utc.ymd(2014, 5, 6).and_hms(23, 59, 59)),
            "2014-05-06T23:59:59Z"
        );
        assert_eq!(
            format!("{:?}", Edt.ymd(2014, 5, 6).and_hms(23, 59, 59)),
            "2014-05-06T23:59:59-04:00"
        );
        assert_eq!(
            format!("{:?}", Kst.ymd(2014, 5, 6).and_hms(23, 59, 59)),
            "2014-05-06T23:59:59+09:00"
        );

        let dt = Utc.ymd(2014, 5, 6).and_hms(7, 8, 9);
        assert_eq!(dt, Edt.ymd(2014, 5, 6).and_hms(3, 8, 9));
        assert_eq!(dt + Duration::seconds(3600 + 60 + 1), Utc.ymd(2014, 5, 6).and_hms(8, 9, 10));
        assert_eq!(
            dt.signed_duration_since(Edt.ymd(2014, 5, 6).and_hms(10, 11, 12)),
            Duration::seconds(-7 * 3600 - 3 * 60 - 3)
        );

        assert_eq!(*Utc.ymd(2014, 5, 6).and_hms(7, 8, 9).offset(), Utc);
        assert_eq!(*Edt.ymd(2014, 5, 6).and_hms(7, 8, 9).offset(), Edt);
        assert!(*Edt.ymd(2014, 5, 6).and_hms(7, 8, 9).offset() != Est);
    }

    #[test]
    fn test_datetime_date_and_time() {
        let tz = FixedOffset::east(5 * 60 * 60);
        let d = tz.ymd(2014, 5, 6).and_hms(7, 8, 9);
        assert_eq!(d.time(), NaiveTime::from_hms(7, 8, 9));
        assert_eq!(d.date(), tz.ymd(2014, 5, 6));
        assert_eq!(d.date().naive_local(), NaiveDate::from_ymd(2014, 5, 6));
        assert_eq!(d.date().and_time(d.time()), Some(d));

        let tz = FixedOffset::east(4 * 60 * 60);
        let d = tz.ymd(2016, 5, 4).and_hms(3, 2, 1);
        assert_eq!(d.time(), NaiveTime::from_hms(3, 2, 1));
        assert_eq!(d.date(), tz.ymd(2016, 5, 4));
        assert_eq!(d.date().naive_local(), NaiveDate::from_ymd(2016, 5, 4));
        assert_eq!(d.date().and_time(d.time()), Some(d));

        let tz = FixedOffset::west(13 * 60 * 60);
        let d = tz.ymd(2017, 8, 9).and_hms(12, 34, 56);
        assert_eq!(d.time(), NaiveTime::from_hms(12, 34, 56));
        assert_eq!(d.date(), tz.ymd(2017, 8, 9));
        assert_eq!(d.date().naive_local(), NaiveDate::from_ymd(2017, 8, 9));
        assert_eq!(d.date().and_time(d.time()), Some(d));

        let utc_d = Utc.ymd(2017, 8, 9).and_hms(12, 34, 56);
        assert!(utc_d < d);
    }

    #[test]
    #[cfg(feature = "clock")]
    fn test_datetime_with_timezone() {
        let local_now = Local::now();
        let utc_now = local_now.with_timezone(&Utc);
        let local_now2 = utc_now.with_timezone(&Local);
        assert_eq!(local_now, local_now2);
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_datetime_rfc2822_and_rfc3339() {
        let EDT = FixedOffset::east(5 * 60 * 60);
        assert_eq!(
            Utc.ymd(2015, 2, 18).and_hms(23, 16, 9).to_rfc2822(),
            "Wed, 18 Feb 2015 23:16:09 +0000"
        );
        assert_eq!(
            Utc.ymd(2015, 2, 18).and_hms(23, 16, 9).to_rfc3339(),
            "2015-02-18T23:16:09+00:00"
        );
        assert_eq!(
            EDT.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150).to_rfc2822(),
            "Wed, 18 Feb 2015 23:16:09 +0500"
        );
        assert_eq!(
            EDT.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150).to_rfc3339(),
            "2015-02-18T23:16:09.150+05:00"
        );
        assert_eq!(
            EDT.ymd(2015, 2, 18).and_hms_micro(23, 59, 59, 1_234_567).to_rfc2822(),
            "Wed, 18 Feb 2015 23:59:60 +0500"
        );
        assert_eq!(
            EDT.ymd(2015, 2, 18).and_hms_micro(23, 59, 59, 1_234_567).to_rfc3339(),
            "2015-02-18T23:59:60.234567+05:00"
        );

        assert_eq!(
            DateTime::parse_from_rfc2822("Wed, 18 Feb 2015 23:16:09 +0000"),
            Ok(FixedOffset::east(0).ymd(2015, 2, 18).and_hms(23, 16, 9))
        );
        assert_eq!(
            DateTime::parse_from_rfc2822("Wed, 18 Feb 2015 23:16:09 -0000"),
            Ok(FixedOffset::east(0).ymd(2015, 2, 18).and_hms(23, 16, 9))
        );
        assert_eq!(
            DateTime::parse_from_rfc3339("2015-02-18T23:16:09Z"),
            Ok(FixedOffset::east(0).ymd(2015, 2, 18).and_hms(23, 16, 9))
        );
        assert_eq!(
            DateTime::parse_from_rfc2822("Wed, 18 Feb 2015 23:59:60 +0500"),
            Ok(EDT.ymd(2015, 2, 18).and_hms_milli(23, 59, 59, 1_000))
        );
        assert_eq!(
            DateTime::parse_from_rfc3339("2015-02-18T23:59:60.234567+05:00"),
            Ok(EDT.ymd(2015, 2, 18).and_hms_micro(23, 59, 59, 1_234_567))
        );
    }

    #[test]
    fn test_rfc3339_opts() {
        use SecondsFormat::*;
        let pst = FixedOffset::east(8 * 60 * 60);
        let dt = pst.ymd(2018, 1, 11).and_hms_nano(10, 5, 13, 084_660_000);
        assert_eq!(dt.to_rfc3339_opts(Secs, false), "2018-01-11T10:05:13+08:00");
        assert_eq!(dt.to_rfc3339_opts(Secs, true), "2018-01-11T10:05:13+08:00");
        assert_eq!(dt.to_rfc3339_opts(Millis, false), "2018-01-11T10:05:13.084+08:00");
        assert_eq!(dt.to_rfc3339_opts(Micros, false), "2018-01-11T10:05:13.084660+08:00");
        assert_eq!(dt.to_rfc3339_opts(Nanos, false), "2018-01-11T10:05:13.084660000+08:00");
        assert_eq!(dt.to_rfc3339_opts(AutoSi, false), "2018-01-11T10:05:13.084660+08:00");

        let ut = DateTime::<Utc>::from_utc(dt.naive_utc(), Utc);
        assert_eq!(ut.to_rfc3339_opts(Secs, false), "2018-01-11T02:05:13+00:00");
        assert_eq!(ut.to_rfc3339_opts(Secs, true), "2018-01-11T02:05:13Z");
        assert_eq!(ut.to_rfc3339_opts(Millis, false), "2018-01-11T02:05:13.084+00:00");
        assert_eq!(ut.to_rfc3339_opts(Millis, true), "2018-01-11T02:05:13.084Z");
        assert_eq!(ut.to_rfc3339_opts(Micros, true), "2018-01-11T02:05:13.084660Z");
        assert_eq!(ut.to_rfc3339_opts(Nanos, true), "2018-01-11T02:05:13.084660000Z");
        assert_eq!(ut.to_rfc3339_opts(AutoSi, true), "2018-01-11T02:05:13.084660Z");
    }

    #[test]
    #[should_panic]
    fn test_rfc3339_opts_nonexhaustive() {
        use SecondsFormat;
        let dt = Utc.ymd(1999, 10, 9).and_hms(1, 2, 3);
        dt.to_rfc3339_opts(SecondsFormat::__NonExhaustive, true);
    }

    #[test]
    fn test_datetime_from_str() {
        assert_eq!(
            "2015-02-18T23:16:9.15Z".parse::<DateTime<FixedOffset>>(),
            Ok(FixedOffset::east(0).ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert_eq!(
            "2015-02-18T23:16:9.15Z".parse::<DateTime<Utc>>(),
            Ok(Utc.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert_eq!(
            "2015-02-18T23:16:9.15 UTC".parse::<DateTime<Utc>>(),
            Ok(Utc.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert_eq!(
            "2015-02-18T23:16:9.15UTC".parse::<DateTime<Utc>>(),
            Ok(Utc.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );

        assert_eq!(
            "2015-2-18T23:16:9.15Z".parse::<DateTime<FixedOffset>>(),
            Ok(FixedOffset::east(0).ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert_eq!(
            "2015-2-18T13:16:9.15-10:00".parse::<DateTime<FixedOffset>>(),
            Ok(FixedOffset::west(10 * 3600).ymd(2015, 2, 18).and_hms_milli(13, 16, 9, 150))
        );
        assert!("2015-2-18T23:16:9.15".parse::<DateTime<FixedOffset>>().is_err());

        assert_eq!(
            "2015-2-18T23:16:9.15Z".parse::<DateTime<Utc>>(),
            Ok(Utc.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert_eq!(
            "2015-2-18T13:16:9.15-10:00".parse::<DateTime<Utc>>(),
            Ok(Utc.ymd(2015, 2, 18).and_hms_milli(23, 16, 9, 150))
        );
        assert!("2015-2-18T23:16:9.15".parse::<DateTime<Utc>>().is_err());

        // no test for `DateTime<Local>`, we cannot verify that much.
    }

    #[test]
    fn test_datetime_parse_from_str() {
        let ymdhms = |y, m, d, h, n, s, off| FixedOffset::east(off).ymd(y, m, d).and_hms(h, n, s);
        assert_eq!(
            DateTime::parse_from_str("2014-5-7T12:34:56+09:30", "%Y-%m-%dT%H:%M:%S%z"),
            Ok(ymdhms(2014, 5, 7, 12, 34, 56, 570 * 60))
        ); // ignore offset
        assert!(DateTime::parse_from_str("20140507000000", "%Y%m%d%H%M%S").is_err()); // no offset
        assert!(DateTime::parse_from_str(
            "Fri, 09 Aug 2013 23:54:35 GMT",
            "%a, %d %b %Y %H:%M:%S GMT"
        )
        .is_err());
        assert_eq!(
            Utc.datetime_from_str("Fri, 09 Aug 2013 23:54:35 GMT", "%a, %d %b %Y %H:%M:%S GMT"),
            Ok(Utc.ymd(2013, 8, 9).and_hms(23, 54, 35))
        );
    }

    #[test]
    fn test_to_string_round_trip() {
        let dt = Utc.ymd(2000, 1, 1).and_hms(0, 0, 0);
        let _dt: DateTime<Utc> = dt.to_string().parse().unwrap();

        let ndt_fixed = dt.with_timezone(&FixedOffset::east(3600));
        let _dt: DateTime<FixedOffset> = ndt_fixed.to_string().parse().unwrap();

        let ndt_fixed = dt.with_timezone(&FixedOffset::east(0));
        let _dt: DateTime<FixedOffset> = ndt_fixed.to_string().parse().unwrap();
    }

    #[test]
    #[cfg(feature = "clock")]
    fn test_to_string_round_trip_with_local() {
        let ndt = Local::now();
        let _dt: DateTime<FixedOffset> = ndt.to_string().parse().unwrap();
    }

    #[test]
    #[cfg(feature = "clock")]
    fn test_datetime_format_with_local() {
        // if we are not around the year boundary, local and UTC date should have the same year
        let dt = Local::now().with_month(5).unwrap();
        assert_eq!(dt.format("%Y").to_string(), dt.with_timezone(&Utc).format("%Y").to_string());
    }

    #[test]
    #[cfg(feature = "clock")]
    fn test_datetime_is_copy() {
        // UTC is known to be `Copy`.
        let a = Utc::now();
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    #[cfg(feature = "clock")]
    fn test_datetime_is_send() {
        use std::thread;

        // UTC is known to be `Send`.
        let a = Utc::now();
        thread::spawn(move || {
            let _ = a;
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_subsecond_part() {
        let datetime = Utc.ymd(2014, 7, 8).and_hms_nano(9, 10, 11, 1234567);

        assert_eq!(1, datetime.timestamp_subsec_millis());
        assert_eq!(1234, datetime.timestamp_subsec_micros());
        assert_eq!(1234567, datetime.timestamp_subsec_nanos());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_from_system_time() {
        use std::time::Duration;

        let epoch = Utc.ymd(1970, 1, 1).and_hms(0, 0, 0);
        let nanos = 999_999_999;

        // SystemTime -> DateTime<Utc>
        assert_eq!(DateTime::<Utc>::from(UNIX_EPOCH), epoch);
        assert_eq!(
            DateTime::<Utc>::from(UNIX_EPOCH + Duration::new(999_999_999, nanos)),
            Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 39, nanos)
        );
        assert_eq!(
            DateTime::<Utc>::from(UNIX_EPOCH - Duration::new(999_999_999, nanos)),
            Utc.ymd(1938, 4, 24).and_hms_nano(22, 13, 20, 1)
        );

        // DateTime<Utc> -> SystemTime
        assert_eq!(SystemTime::from(epoch), UNIX_EPOCH);
        assert_eq!(
            SystemTime::from(Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 39, nanos)),
            UNIX_EPOCH + Duration::new(999_999_999, nanos)
        );
        assert_eq!(
            SystemTime::from(Utc.ymd(1938, 4, 24).and_hms_nano(22, 13, 20, 1)),
            UNIX_EPOCH - Duration::new(999_999_999, 999_999_999)
        );

        // DateTime<any tz> -> SystemTime (via `with_timezone`)
        #[cfg(feature = "clock")]
        {
            assert_eq!(SystemTime::from(epoch.with_timezone(&Local)), UNIX_EPOCH);
        }
        assert_eq!(SystemTime::from(epoch.with_timezone(&FixedOffset::east(32400))), UNIX_EPOCH);
        assert_eq!(SystemTime::from(epoch.with_timezone(&FixedOffset::west(28800))), UNIX_EPOCH);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_from_system_time() {
        use std::time::Duration;

        let nanos = 999_999_000;

        let epoch = Utc.ymd(1970, 1, 1).and_hms(0, 0, 0);

        // SystemTime -> DateTime<Utc>
        assert_eq!(DateTime::<Utc>::from(UNIX_EPOCH), epoch);
        assert_eq!(
            DateTime::<Utc>::from(UNIX_EPOCH + Duration::new(999_999_999, nanos)),
            Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 39, nanos)
        );
        assert_eq!(
            DateTime::<Utc>::from(UNIX_EPOCH - Duration::new(999_999_999, nanos)),
            Utc.ymd(1938, 4, 24).and_hms_nano(22, 13, 20, 1_000)
        );

        // DateTime<Utc> -> SystemTime
        assert_eq!(SystemTime::from(epoch), UNIX_EPOCH);
        assert_eq!(
            SystemTime::from(Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 39, nanos)),
            UNIX_EPOCH + Duration::new(999_999_999, nanos)
        );
        assert_eq!(
            SystemTime::from(Utc.ymd(1938, 4, 24).and_hms_nano(22, 13, 20, 1_000)),
            UNIX_EPOCH - Duration::new(999_999_999, nanos)
        );

        // DateTime<any tz> -> SystemTime (via `with_timezone`)
        #[cfg(feature = "clock")]
        {
            assert_eq!(SystemTime::from(epoch.with_timezone(&Local)), UNIX_EPOCH);
        }
        assert_eq!(SystemTime::from(epoch.with_timezone(&FixedOffset::east(32400))), UNIX_EPOCH);
        assert_eq!(SystemTime::from(epoch.with_timezone(&FixedOffset::west(28800))), UNIX_EPOCH);
    }

    #[test]
    fn test_datetime_format_alignment() {
        let datetime = Utc.ymd(2007, 01, 02);

        // Item::Literal
        let percent = datetime.format("%%");
        assert_eq!("  %", format!("{:>3}", percent));
        assert_eq!("%  ", format!("{:<3}", percent));
        assert_eq!(" % ", format!("{:^3}", percent));

        // Item::Numeric
        let year = datetime.format("%Y");
        assert_eq!("  2007", format!("{:>6}", year));
        assert_eq!("2007  ", format!("{:<6}", year));
        assert_eq!(" 2007 ", format!("{:^6}", year));

        // Item::Fixed
        let tz = datetime.format("%Z");
        assert_eq!("  UTC", format!("{:>5}", tz));
        assert_eq!("UTC  ", format!("{:<5}", tz));
        assert_eq!(" UTC ", format!("{:^5}", tz));

        // [Item::Numeric, Item::Space, Item::Literal, Item::Space, Item::Numeric]
        let ymd = datetime.format("%Y %B %d");
        let ymd_formatted = "2007 January 02";
        assert_eq!(format!("  {}", ymd_formatted), format!("{:>17}", ymd));
        assert_eq!(format!("{}  ", ymd_formatted), format!("{:<17}", ymd));
        assert_eq!(format!(" {} ", ymd_formatted), format!("{:^17}", ymd));
    }
}
