// This is a part of Chrono.
// See README.md and LICENSE.txt for details.

//! ISO 8601 calendar date with time zone.

#[cfg(any(feature = "alloc", feature = "std", test))]
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::ops::{Add, Sub};
use core::{fmt, hash};
use oldtime::Duration as OldDuration;

#[cfg(feature = "unstable-locales")]
use format::Locale;
#[cfg(any(feature = "alloc", feature = "std", test))]
use format::{DelayedFormat, Item, StrftimeItems};
use naive::{self, IsoWeek, NaiveDate, NaiveTime};
use offset::{TimeZone, Utc};
use DateTime;
use {Datelike, Weekday};

/// ISO 8601 calendar date with time zone.
///
/// This type should be considered ambiguous at best,
/// due to the inherent lack of precision required for the time zone resolution.
/// For serialization and deserialization uses, it is best to use `NaiveDate` instead.
/// There are some guarantees on the usage of `Date<Tz>`:
///
/// - If properly constructed via `TimeZone::ymd` and others without an error,
///   the corresponding local date should exist for at least a moment.
///   (It may still have a gap from the offset changes.)
///
/// - The `TimeZone` is free to assign *any* `Offset` to the local date,
///   as long as that offset did occur in given day.
///   For example, if `2015-03-08T01:59-08:00` is followed by `2015-03-08T03:00-07:00`,
///   it may produce either `2015-03-08-08:00` or `2015-03-08-07:00`
///   but *not* `2015-03-08+00:00` and others.
///
/// - Once constructed as a full `DateTime`,
///   `DateTime::date` and other associated methods should return those for the original `Date`.
///   For example, if `dt = tz.ymd(y,m,d).hms(h,n,s)` were valid, `dt.date() == tz.ymd(y,m,d)`.
///
/// - The date is timezone-agnostic up to one day (i.e. practically always),
///   so the local date and UTC date should be equal for most cases
///   even though the raw calculation between `NaiveDate` and `Duration` may not.
#[derive(Clone)]
pub struct Date<Tz: TimeZone> {
    date: NaiveDate,
    offset: Tz::Offset,
}

/// The minimum possible `Date`.
pub const MIN_DATE: Date<Utc> = Date { date: naive::MIN_DATE, offset: Utc };
/// The maximum possible `Date`.
pub const MAX_DATE: Date<Utc> = Date { date: naive::MAX_DATE, offset: Utc };

impl<Tz: TimeZone> Date<Tz> {
    /// Makes a new `Date` with given *UTC* date and offset.
    /// The local date should be constructed via the `TimeZone` trait.
    //
    // note: this constructor is purposely not named to `new` to discourage the direct usage.
    #[inline]
    pub fn from_utc(date: NaiveDate, offset: Tz::Offset) -> Date<Tz> {
        Date { date: date, offset: offset }
    }

    /// Makes a new `DateTime` from the current date and given `NaiveTime`.
    /// The offset in the current date is preserved.
    ///
    /// Panics on invalid datetime.
    #[inline]
    pub fn and_time(&self, time: NaiveTime) -> Option<DateTime<Tz>> {
        let localdt = self.naive_local().and_time(time);
        self.timezone().from_local_datetime(&localdt).single()
    }

    /// Makes a new `DateTime` from the current date, hour, minute and second.
    /// The offset in the current date is preserved.
    ///
    /// Panics on invalid hour, minute and/or second.
    #[inline]
    pub fn and_hms(&self, hour: u32, min: u32, sec: u32) -> DateTime<Tz> {
        self.and_hms_opt(hour, min, sec).expect("invalid time")
    }

    /// Makes a new `DateTime` from the current date, hour, minute and second.
    /// The offset in the current date is preserved.
    ///
    /// Returns `None` on invalid hour, minute and/or second.
    #[inline]
    pub fn and_hms_opt(&self, hour: u32, min: u32, sec: u32) -> Option<DateTime<Tz>> {
        NaiveTime::from_hms_opt(hour, min, sec).and_then(|time| self.and_time(time))
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and millisecond.
    /// The millisecond part can exceed 1,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Panics on invalid hour, minute, second and/or millisecond.
    #[inline]
    pub fn and_hms_milli(&self, hour: u32, min: u32, sec: u32, milli: u32) -> DateTime<Tz> {
        self.and_hms_milli_opt(hour, min, sec, milli).expect("invalid time")
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and millisecond.
    /// The millisecond part can exceed 1,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Returns `None` on invalid hour, minute, second and/or millisecond.
    #[inline]
    pub fn and_hms_milli_opt(
        &self,
        hour: u32,
        min: u32,
        sec: u32,
        milli: u32,
    ) -> Option<DateTime<Tz>> {
        NaiveTime::from_hms_milli_opt(hour, min, sec, milli).and_then(|time| self.and_time(time))
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and microsecond.
    /// The microsecond part can exceed 1,000,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Panics on invalid hour, minute, second and/or microsecond.
    #[inline]
    pub fn and_hms_micro(&self, hour: u32, min: u32, sec: u32, micro: u32) -> DateTime<Tz> {
        self.and_hms_micro_opt(hour, min, sec, micro).expect("invalid time")
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and microsecond.
    /// The microsecond part can exceed 1,000,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Returns `None` on invalid hour, minute, second and/or microsecond.
    #[inline]
    pub fn and_hms_micro_opt(
        &self,
        hour: u32,
        min: u32,
        sec: u32,
        micro: u32,
    ) -> Option<DateTime<Tz>> {
        NaiveTime::from_hms_micro_opt(hour, min, sec, micro).and_then(|time| self.and_time(time))
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and nanosecond.
    /// The nanosecond part can exceed 1,000,000,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Panics on invalid hour, minute, second and/or nanosecond.
    #[inline]
    pub fn and_hms_nano(&self, hour: u32, min: u32, sec: u32, nano: u32) -> DateTime<Tz> {
        self.and_hms_nano_opt(hour, min, sec, nano).expect("invalid time")
    }

    /// Makes a new `DateTime` from the current date, hour, minute, second and nanosecond.
    /// The nanosecond part can exceed 1,000,000,000 in order to represent the leap second.
    /// The offset in the current date is preserved.
    ///
    /// Returns `None` on invalid hour, minute, second and/or nanosecond.
    #[inline]
    pub fn and_hms_nano_opt(
        &self,
        hour: u32,
        min: u32,
        sec: u32,
        nano: u32,
    ) -> Option<DateTime<Tz>> {
        NaiveTime::from_hms_nano_opt(hour, min, sec, nano).and_then(|time| self.and_time(time))
    }

    /// Makes a new `Date` for the next date.
    ///
    /// Panics when `self` is the last representable date.
    #[inline]
    pub fn succ(&self) -> Date<Tz> {
        self.succ_opt().expect("out of bound")
    }

    /// Makes a new `Date` for the next date.
    ///
    /// Returns `None` when `self` is the last representable date.
    #[inline]
    pub fn succ_opt(&self) -> Option<Date<Tz>> {
        self.date.succ_opt().map(|date| Date::from_utc(date, self.offset.clone()))
    }

    /// Makes a new `Date` for the prior date.
    ///
    /// Panics when `self` is the first representable date.
    #[inline]
    pub fn pred(&self) -> Date<Tz> {
        self.pred_opt().expect("out of bound")
    }

    /// Makes a new `Date` for the prior date.
    ///
    /// Returns `None` when `self` is the first representable date.
    #[inline]
    pub fn pred_opt(&self) -> Option<Date<Tz>> {
        self.date.pred_opt().map(|date| Date::from_utc(date, self.offset.clone()))
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
    /// This does not change the actual `Date` (but will change the string representation).
    #[inline]
    pub fn with_timezone<Tz2: TimeZone>(&self, tz: &Tz2) -> Date<Tz2> {
        tz.from_utc_date(&self.date)
    }

    /// Adds given `Duration` to the current date.
    ///
    /// Returns `None` when it will result in overflow.
    #[inline]
    pub fn checked_add_signed(self, rhs: OldDuration) -> Option<Date<Tz>> {
        let date = try_opt!(self.date.checked_add_signed(rhs));
        Some(Date { date: date, offset: self.offset })
    }

    /// Subtracts given `Duration` from the current date.
    ///
    /// Returns `None` when it will result in overflow.
    #[inline]
    pub fn checked_sub_signed(self, rhs: OldDuration) -> Option<Date<Tz>> {
        let date = try_opt!(self.date.checked_sub_signed(rhs));
        Some(Date { date: date, offset: self.offset })
    }

    /// Subtracts another `Date` from the current date.
    /// Returns a `Duration` of integral numbers.
    ///
    /// This does not overflow or underflow at all,
    /// as all possible output fits in the range of `Duration`.
    #[inline]
    pub fn signed_duration_since<Tz2: TimeZone>(self, rhs: Date<Tz2>) -> OldDuration {
        self.date.signed_duration_since(rhs.date)
    }

    /// Returns a view to the naive UTC date.
    #[inline]
    pub fn naive_utc(&self) -> NaiveDate {
        self.date
    }

    /// Returns a view to the naive local date.
    ///
    /// This is technically the same as [`naive_utc`](#method.naive_utc)
    /// because the offset is restricted to never exceed one day,
    /// but provided for the consistency.
    #[inline]
    pub fn naive_local(&self) -> NaiveDate {
        self.date
    }
}

/// Maps the local date to other date with given conversion function.
fn map_local<Tz: TimeZone, F>(d: &Date<Tz>, mut f: F) -> Option<Date<Tz>>
where
    F: FnMut(NaiveDate) -> Option<NaiveDate>,
{
    f(d.naive_local()).and_then(|date| d.timezone().from_local_date(&date).single())
}

impl<Tz: TimeZone> Date<Tz>
where
    Tz::Offset: fmt::Display,
{
    /// Formats the date with the specified formatting items.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format_with_items<'a, I, B>(&self, items: I) -> DelayedFormat<I>
    where
        I: Iterator<Item = B> + Clone,
        B: Borrow<Item<'a>>,
    {
        DelayedFormat::new_with_offset(Some(self.naive_local()), None, &self.offset, items)
    }

    /// Formats the date with the specified format string.
    /// See the [`format::strftime` module](./format/strftime/index.html)
    /// on the supported escape sequences.
    #[cfg(any(feature = "alloc", feature = "std", test))]
    #[inline]
    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.format_with_items(StrftimeItems::new(fmt))
    }

    /// Formats the date with the specified formatting items and locale.
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
        DelayedFormat::new_with_offset_and_locale(
            Some(self.naive_local()),
            None,
            &self.offset,
            items,
            locale,
        )
    }

    /// Formats the date with the specified format string and locale.
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

impl<Tz: TimeZone> Datelike for Date<Tz> {
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
    fn with_year(&self, year: i32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_year(year))
    }

    #[inline]
    fn with_month(&self, month: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_month(month))
    }

    #[inline]
    fn with_month0(&self, month0: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_month0(month0))
    }

    #[inline]
    fn with_day(&self, day: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_day(day))
    }

    #[inline]
    fn with_day0(&self, day0: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_day0(day0))
    }

    #[inline]
    fn with_ordinal(&self, ordinal: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_ordinal(ordinal))
    }

    #[inline]
    fn with_ordinal0(&self, ordinal0: u32) -> Option<Date<Tz>> {
        map_local(self, |date| date.with_ordinal0(ordinal0))
    }
}

// we need them as automatic impls cannot handle associated types
impl<Tz: TimeZone> Copy for Date<Tz> where <Tz as TimeZone>::Offset: Copy {}
unsafe impl<Tz: TimeZone> Send for Date<Tz> where <Tz as TimeZone>::Offset: Send {}

impl<Tz: TimeZone, Tz2: TimeZone> PartialEq<Date<Tz2>> for Date<Tz> {
    fn eq(&self, other: &Date<Tz2>) -> bool {
        self.date == other.date
    }
}

impl<Tz: TimeZone> Eq for Date<Tz> {}

impl<Tz: TimeZone> PartialOrd for Date<Tz> {
    fn partial_cmp(&self, other: &Date<Tz>) -> Option<Ordering> {
        self.date.partial_cmp(&other.date)
    }
}

impl<Tz: TimeZone> Ord for Date<Tz> {
    fn cmp(&self, other: &Date<Tz>) -> Ordering {
        self.date.cmp(&other.date)
    }
}

impl<Tz: TimeZone> hash::Hash for Date<Tz> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.date.hash(state)
    }
}

impl<Tz: TimeZone> Add<OldDuration> for Date<Tz> {
    type Output = Date<Tz>;

    #[inline]
    fn add(self, rhs: OldDuration) -> Date<Tz> {
        self.checked_add_signed(rhs).expect("`Date + Duration` overflowed")
    }
}

impl<Tz: TimeZone> Sub<OldDuration> for Date<Tz> {
    type Output = Date<Tz>;

    #[inline]
    fn sub(self, rhs: OldDuration) -> Date<Tz> {
        self.checked_sub_signed(rhs).expect("`Date - Duration` overflowed")
    }
}

impl<Tz: TimeZone> Sub<Date<Tz>> for Date<Tz> {
    type Output = OldDuration;

    #[inline]
    fn sub(self, rhs: Date<Tz>) -> OldDuration {
        self.signed_duration_since(rhs)
    }
}

impl<Tz: TimeZone> fmt::Debug for Date<Tz> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}{:?}", self.naive_local(), self.offset)
    }
}

impl<Tz: TimeZone> fmt::Display for Date<Tz>
where
    Tz::Offset: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.naive_local(), self.offset)
    }
}
