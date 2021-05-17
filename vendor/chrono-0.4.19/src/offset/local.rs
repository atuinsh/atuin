// This is a part of Chrono.
// See README.md and LICENSE.txt for details.

//! The local (system) time zone.

#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
use sys::{self, Timespec};

use super::fixed::FixedOffset;
use super::{LocalResult, TimeZone};
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
use naive::NaiveTime;
use naive::{NaiveDate, NaiveDateTime};
use {Date, DateTime};
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
use {Datelike, Timelike};

/// Converts a `time::Tm` struct into the timezone-aware `DateTime`.
/// This assumes that `time` is working correctly, i.e. any error is fatal.
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
fn tm_to_datetime(mut tm: sys::Tm) -> DateTime<Local> {
    if tm.tm_sec >= 60 {
        tm.tm_nsec += (tm.tm_sec - 59) * 1_000_000_000;
        tm.tm_sec = 59;
    }

    #[cfg(not(windows))]
    fn tm_to_naive_date(tm: &sys::Tm) -> NaiveDate {
        // from_yo is more efficient than from_ymd (since it's the internal representation).
        NaiveDate::from_yo(tm.tm_year + 1900, tm.tm_yday as u32 + 1)
    }

    #[cfg(windows)]
    fn tm_to_naive_date(tm: &sys::Tm) -> NaiveDate {
        // ...but tm_yday is broken in Windows (issue #85)
        NaiveDate::from_ymd(tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
    }

    let date = tm_to_naive_date(&tm);
    let time = NaiveTime::from_hms_nano(
        tm.tm_hour as u32,
        tm.tm_min as u32,
        tm.tm_sec as u32,
        tm.tm_nsec as u32,
    );
    let offset = FixedOffset::east(tm.tm_utcoff);
    DateTime::from_utc(date.and_time(time) - offset, offset)
}

/// Converts a local `NaiveDateTime` to the `time::Timespec`.
#[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
fn datetime_to_timespec(d: &NaiveDateTime, local: bool) -> sys::Timespec {
    // well, this exploits an undocumented `Tm::to_timespec` behavior
    // to get the exact function we want (either `timegm` or `mktime`).
    // the number 1 is arbitrary but should be non-zero to trigger `mktime`.
    let tm_utcoff = if local { 1 } else { 0 };

    let tm = sys::Tm {
        tm_sec: d.second() as i32,
        tm_min: d.minute() as i32,
        tm_hour: d.hour() as i32,
        tm_mday: d.day() as i32,
        tm_mon: d.month0() as i32, // yes, C is that strange...
        tm_year: d.year() - 1900,  // this doesn't underflow, we know that d is `NaiveDateTime`.
        tm_wday: 0,                // to_local ignores this
        tm_yday: 0,                // and this
        tm_isdst: -1,
        tm_utcoff: tm_utcoff,
        // do not set this, OS APIs are heavily inconsistent in terms of leap second handling
        tm_nsec: 0,
    };

    tm.to_timespec()
}

/// The local timescale. This is implemented via the standard `time` crate.
///
/// Using the [`TimeZone`](./trait.TimeZone.html) methods
/// on the Local struct is the preferred way to construct `DateTime<Local>`
/// instances.
///
/// # Example
///
/// ~~~~
/// use chrono::{Local, DateTime, TimeZone};
///
/// let dt: DateTime<Local> = Local::now();
/// let dt: DateTime<Local> = Local.timestamp(0, 0);
/// ~~~~
#[derive(Copy, Clone, Debug)]
pub struct Local;

impl Local {
    /// Returns a `Date` which corresponds to the current date.
    pub fn today() -> Date<Local> {
        Local::now().date()
    }

    /// Returns a `DateTime` which corresponds to the current date.
    #[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
    pub fn now() -> DateTime<Local> {
        tm_to_datetime(Timespec::now().local())
    }

    /// Returns a `DateTime` which corresponds to the current date.
    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
    pub fn now() -> DateTime<Local> {
        use super::Utc;
        let now: DateTime<Utc> = super::Utc::now();

        // Workaround missing timezone logic in `time` crate
        let offset = FixedOffset::west((js_sys::Date::new_0().get_timezone_offset() as i32) * 60);
        DateTime::from_utc(now.naive_utc(), offset)
    }
}

impl TimeZone for Local {
    type Offset = FixedOffset;

    fn from_offset(_offset: &FixedOffset) -> Local {
        Local
    }

    // they are easier to define in terms of the finished date and time unlike other offsets
    fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<FixedOffset> {
        self.from_local_date(local).map(|date| *date.offset())
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
        self.from_local_datetime(local).map(|datetime| *datetime.offset())
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
        *self.from_utc_date(utc).offset()
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
        *self.from_utc_datetime(utc).offset()
    }

    // override them for avoiding redundant works
    fn from_local_date(&self, local: &NaiveDate) -> LocalResult<Date<Local>> {
        // this sounds very strange, but required for keeping `TimeZone::ymd` sane.
        // in the other words, we use the offset at the local midnight
        // but keep the actual date unaltered (much like `FixedOffset`).
        let midnight = self.from_local_datetime(&local.and_hms(0, 0, 0));
        midnight.map(|datetime| Date::from_utc(*local, *datetime.offset()))
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
    fn from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<DateTime<Local>> {
        let mut local = local.clone();
        // Get the offset from the js runtime
        let offset = FixedOffset::west((js_sys::Date::new_0().get_timezone_offset() as i32) * 60);
        local -= ::Duration::seconds(offset.local_minus_utc() as i64);
        LocalResult::Single(DateTime::from_utc(local, offset))
    }

    #[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
    fn from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<DateTime<Local>> {
        let timespec = datetime_to_timespec(local, true);

        // datetime_to_timespec completely ignores leap seconds, so we need to adjust for them
        let mut tm = timespec.local();
        assert_eq!(tm.tm_nsec, 0);
        tm.tm_nsec = local.nanosecond() as i32;

        LocalResult::Single(tm_to_datetime(tm))
    }

    fn from_utc_date(&self, utc: &NaiveDate) -> Date<Local> {
        let midnight = self.from_utc_datetime(&utc.and_hms(0, 0, 0));
        Date::from_utc(*utc, *midnight.offset())
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind"))]
    fn from_utc_datetime(&self, utc: &NaiveDateTime) -> DateTime<Local> {
        // Get the offset from the js runtime
        let offset = FixedOffset::west((js_sys::Date::new_0().get_timezone_offset() as i32) * 60);
        DateTime::from_utc(*utc, offset)
    }

    #[cfg(not(all(target_arch = "wasm32", not(target_os = "wasi"), feature = "wasmbind")))]
    fn from_utc_datetime(&self, utc: &NaiveDateTime) -> DateTime<Local> {
        let timespec = datetime_to_timespec(utc, false);

        // datetime_to_timespec completely ignores leap seconds, so we need to adjust for them
        let mut tm = timespec.local();
        assert_eq!(tm.tm_nsec, 0);
        tm.tm_nsec = utc.nanosecond() as i32;

        tm_to_datetime(tm)
    }
}

#[cfg(test)]
mod tests {
    use super::Local;
    use offset::TimeZone;
    use Datelike;

    #[test]
    fn test_local_date_sanity_check() {
        // issue #27
        assert_eq!(Local.ymd(2999, 12, 28).day(), 28);
    }

    #[test]
    fn test_leap_second() {
        // issue #123
        let today = Local::today();

        let dt = today.and_hms_milli(1, 2, 59, 1000);
        let timestr = dt.time().to_string();
        // the OS API may or may not support the leap second,
        // but there are only two sensible options.
        assert!(timestr == "01:02:60" || timestr == "01:03:00", "unexpected timestr {:?}", timestr);

        let dt = today.and_hms_milli(1, 2, 3, 1234);
        let timestr = dt.time().to_string();
        assert!(
            timestr == "01:02:03.234" || timestr == "01:02:04.234",
            "unexpected timestr {:?}",
            timestr
        );
    }
}
