// Copyright 2012-2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Simple time handling.
//!
//! # Usage
//!
//! This crate is [on crates.io](https://crates.io/crates/time) and can be
//! used by adding `time` to the dependencies in your project's `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! time = "0.1"
//! ```
//!
//! And this in your crate root:
//!
//! ```rust
//! extern crate time;
//! ```
//!
//! This crate uses the same syntax for format strings as the
//! [`strftime()`](http://man7.org/linux/man-pages/man3/strftime.3.html)
//! function from the C standard library.

#![doc(html_logo_url = "https://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "https://www.rust-lang.org/favicon.ico",
       html_root_url = "https://doc.rust-lang.org/time/")]
#![allow(unknown_lints)]
#![allow(ellipsis_inclusive_range_patterns)] // `..=` requires Rust 1.26
#![allow(trivial_numeric_casts)]
#![cfg_attr(test, deny(warnings))]

#[cfg(unix)] extern crate libc;
#[cfg(windows)] extern crate winapi;
#[cfg(feature = "rustc-serialize")] extern crate rustc_serialize;

#[cfg(test)] #[macro_use] extern crate log;

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::ops::{Add, Sub};

pub use duration::{Duration, OutOfRangeError};

use self::ParseError::{InvalidDay, InvalidDayOfMonth, InvalidDayOfWeek,
                       InvalidDayOfYear, InvalidFormatSpecifier, InvalidHour,
                       InvalidMinute, InvalidMonth, InvalidSecond, InvalidTime,
                       InvalidYear, InvalidZoneOffset, InvalidSecondsSinceEpoch,
                       MissingFormatConverter, UnexpectedCharacter};

pub use parse::strptime;

mod display;
mod duration;
mod parse;
mod sys;

static NSEC_PER_SEC: i32 = 1_000_000_000;

/// A record specifying a time value in seconds and nanoseconds, where
/// nanoseconds represent the offset from the given second.
///
/// For example a timespec of 1.2 seconds after the beginning of the epoch would
/// be represented as {sec: 1, nsec: 200000000}.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[cfg_attr(feature = "rustc-serialize", derive(RustcEncodable, RustcDecodable))]
pub struct Timespec { pub sec: i64, pub nsec: i32 }
/*
 * Timespec assumes that pre-epoch Timespecs have negative sec and positive
 * nsec fields. Darwin's and Linux's struct timespec functions handle pre-
 * epoch timestamps using a "two steps back, one step forward" representation,
 * though the man pages do not actually document this. For example, the time
 * -1.2 seconds before the epoch is represented by `Timespec { sec: -2_i64,
 * nsec: 800_000_000 }`.
 */
impl Timespec {
    pub fn new(sec: i64, nsec: i32) -> Timespec {
        assert!(nsec >= 0 && nsec < NSEC_PER_SEC);
        Timespec { sec: sec, nsec: nsec }
    }
}

impl Add<Duration> for Timespec {
    type Output = Timespec;

    fn add(self, other: Duration) -> Timespec {
        let d_sec = other.num_seconds();
        // It is safe to unwrap the nanoseconds, because there cannot be
        // more than one second left, which fits in i64 and in i32.
        let d_nsec = (other - Duration::seconds(d_sec))
                     .num_nanoseconds().unwrap() as i32;
        let mut sec = self.sec + d_sec;
        let mut nsec = self.nsec + d_nsec;
        if nsec >= NSEC_PER_SEC {
            nsec -= NSEC_PER_SEC;
            sec += 1;
        } else if nsec < 0 {
            nsec += NSEC_PER_SEC;
            sec -= 1;
        }
        Timespec::new(sec, nsec)
    }
}

impl Sub<Duration> for Timespec {
    type Output = Timespec;

    fn sub(self, other: Duration) -> Timespec {
        let d_sec = other.num_seconds();
        // It is safe to unwrap the nanoseconds, because there cannot be
        // more than one second left, which fits in i64 and in i32.
        let d_nsec = (other - Duration::seconds(d_sec))
                     .num_nanoseconds().unwrap() as i32;
        let mut sec = self.sec - d_sec;
        let mut nsec = self.nsec - d_nsec;
        if nsec >= NSEC_PER_SEC {
            nsec -= NSEC_PER_SEC;
            sec += 1;
        } else if nsec < 0 {
            nsec += NSEC_PER_SEC;
            sec -= 1;
        }
        Timespec::new(sec, nsec)
    }
}

impl Sub<Timespec> for Timespec {
    type Output = Duration;

    fn sub(self, other: Timespec) -> Duration {
        let sec = self.sec - other.sec;
        let nsec = self.nsec - other.nsec;
        Duration::seconds(sec) + Duration::nanoseconds(nsec as i64)
    }
}

/**
 * Returns the current time as a `timespec` containing the seconds and
 * nanoseconds since 1970-01-01T00:00:00Z.
 */
pub fn get_time() -> Timespec {
    let (sec, nsec) = sys::get_time();
    Timespec::new(sec, nsec)
}


/**
 * Returns the current value of a high-resolution performance counter
 * in nanoseconds since an unspecified epoch.
 */
#[inline]
pub fn precise_time_ns() -> u64 {
    sys::get_precise_ns()
}


/**
 * Returns the current value of a high-resolution performance counter
 * in seconds since an unspecified epoch.
 */
pub fn precise_time_s() -> f64 {
    return (precise_time_ns() as f64) / 1000000000.;
}

/// An opaque structure representing a moment in time.
///
/// The only operation that can be performed on a `PreciseTime` is the
/// calculation of the `Duration` of time that lies between them.
///
/// # Examples
///
/// Repeatedly call a function for 1 second:
///
/// ```rust
/// use time::{Duration, PreciseTime};
/// # fn do_some_work() {}
///
/// let start = PreciseTime::now();
///
/// while start.to(PreciseTime::now()) < Duration::seconds(1) {
///     do_some_work();
/// }
/// ```
#[derive(Copy, Clone)]
pub struct PreciseTime(u64);

impl PreciseTime {
    /// Returns a `PreciseTime` representing the current moment in time.
    pub fn now() -> PreciseTime {
        PreciseTime(precise_time_ns())
    }

    /// Returns a `Duration` representing the span of time from the value of
    /// `self` to the value of `later`.
    ///
    /// # Notes
    ///
    /// If `later` represents a time before `self`, the result of this method
    /// is unspecified.
    ///
    /// If `later` represents a time more than 293 years after `self`, the
    /// result of this method is unspecified.
    #[inline]
    pub fn to(&self, later: PreciseTime) -> Duration {
        // NB: even if later is less than self due to overflow, this will work
        // since the subtraction will underflow properly as well.
        //
        // We could deal with the overflow when casting to an i64, but all that
        // gets us is the ability to handle intervals of up to 584 years, which
        // seems not very useful :)
        Duration::nanoseconds((later.0 - self.0) as i64)
    }
}

/// A structure representing a moment in time.
///
/// `SteadyTime`s are generated by a "steady" clock, that is, a clock which
/// never experiences discontinuous jumps and for which time always flows at
/// the same rate.
///
/// # Examples
///
/// Repeatedly call a function for 1 second:
///
/// ```rust
/// # use time::{Duration, SteadyTime};
/// # fn do_some_work() {}
/// let start = SteadyTime::now();
///
/// while SteadyTime::now() - start < Duration::seconds(1) {
///     do_some_work();
/// }
/// ```
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug)]
pub struct SteadyTime(sys::SteadyTime);

impl SteadyTime {
    /// Returns a `SteadyTime` representing the current moment in time.
    pub fn now() -> SteadyTime {
        SteadyTime(sys::SteadyTime::now())
    }
}

impl fmt::Display for SteadyTime {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        // TODO: needs a display customization
        fmt::Debug::fmt(self, fmt)
    }
}

impl Sub for SteadyTime {
    type Output = Duration;

    fn sub(self, other: SteadyTime) -> Duration {
        self.0 - other.0
    }
}

impl Sub<Duration> for SteadyTime {
    type Output = SteadyTime;

    fn sub(self, other: Duration) -> SteadyTime {
        SteadyTime(self.0 - other)
    }
}

impl Add<Duration> for SteadyTime {
    type Output = SteadyTime;

    fn add(self, other: Duration) -> SteadyTime {
        SteadyTime(self.0 + other)
    }
}

#[cfg(not(any(windows, target_env = "sgx")))]
pub fn tzset() {
    extern { fn tzset(); }
    unsafe { tzset() }
}


#[cfg(any(windows, target_env = "sgx"))]
pub fn tzset() {}

/// Holds a calendar date and time broken down into its components (year, month,
/// day, and so on), also called a broken-down time value.
// FIXME: use c_int instead of i32?
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "rustc-serialize", derive(RustcEncodable, RustcDecodable))]
pub struct Tm {
    /// Seconds after the minute - [0, 60]
    pub tm_sec: i32,

    /// Minutes after the hour - [0, 59]
    pub tm_min: i32,

    /// Hours after midnight - [0, 23]
    pub tm_hour: i32,

    /// Day of the month - [1, 31]
    pub tm_mday: i32,

    /// Months since January - [0, 11]
    pub tm_mon: i32,

    /// Years since 1900
    pub tm_year: i32,

    /// Days since Sunday - [0, 6]. 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
    pub tm_wday: i32,

    /// Days since January 1 - [0, 365]
    pub tm_yday: i32,

    /// Daylight Saving Time flag.
    ///
    /// This value is positive if Daylight Saving Time is in effect, zero if
    /// Daylight Saving Time is not in effect, and negative if this information
    /// is not available.
    pub tm_isdst: i32,

    /// Identifies the time zone that was used to compute this broken-down time
    /// value, including any adjustment for Daylight Saving Time. This is the
    /// number of seconds east of UTC. For example, for U.S. Pacific Daylight
    /// Time, the value is `-7*60*60 = -25200`.
    pub tm_utcoff: i32,

    /// Nanoseconds after the second - [0, 10<sup>9</sup> - 1]
    pub tm_nsec: i32,
}

impl Add<Duration> for Tm {
    type Output = Tm;

    /// The resulting Tm is in UTC.
    // FIXME:  The resulting Tm should have the same timezone as `self`;
    // however, we need a function such as `at_tm(clock: Timespec, offset: i32)`
    // for this.
    fn add(self, other: Duration) -> Tm {
        at_utc(self.to_timespec() + other)
    }
}

impl Sub<Duration> for Tm {
    type Output = Tm;

    /// The resulting Tm is in UTC.
    // FIXME:  The resulting Tm should have the same timezone as `self`;
    // however, we need a function such as `at_tm(clock: Timespec, offset: i32)`
    // for this.
    fn sub(self, other: Duration) -> Tm {
        at_utc(self.to_timespec() - other)
    }
}

impl Sub<Tm> for Tm {
    type Output = Duration;

    fn sub(self, other: Tm) -> Duration {
        self.to_timespec() - other.to_timespec()
    }
}

impl PartialOrd for Tm {
    fn partial_cmp(&self, other: &Tm) -> Option<Ordering> {
        self.to_timespec().partial_cmp(&other.to_timespec())
    }
}

impl Ord for Tm {
    fn cmp(&self, other: &Tm) -> Ordering {
        self.to_timespec().cmp(&other.to_timespec())
    }
}

pub fn empty_tm() -> Tm {
    Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_utcoff: 0,
        tm_nsec: 0,
    }
}

/// Returns the specified time in UTC
pub fn at_utc(clock: Timespec) -> Tm {
    let Timespec { sec, nsec } = clock;
    let mut tm = empty_tm();
    sys::time_to_utc_tm(sec, &mut tm);
    tm.tm_nsec = nsec;
    tm
}

/// Returns the current time in UTC
pub fn now_utc() -> Tm {
    at_utc(get_time())
}

/// Returns the specified time in the local timezone
pub fn at(clock: Timespec) -> Tm {
    let Timespec { sec, nsec } = clock;
    let mut tm = empty_tm();
    sys::time_to_local_tm(sec, &mut tm);
    tm.tm_nsec = nsec;
    tm
}

/// Returns the current time in the local timezone
pub fn now() -> Tm {
    at(get_time())
}

impl Tm {
    /// Convert time to the seconds from January 1, 1970
    pub fn to_timespec(&self) -> Timespec {
        let sec = match self.tm_utcoff {
            0 => sys::utc_tm_to_time(self),
            _ => sys::local_tm_to_time(self)
        };

        Timespec::new(sec, self.tm_nsec)
    }

    /// Convert time to the local timezone
    pub fn to_local(&self) -> Tm {
        at(self.to_timespec())
    }

    /// Convert time to the UTC
    pub fn to_utc(&self) -> Tm {
        match self.tm_utcoff {
            0 => *self,
            _ => at_utc(self.to_timespec())
        }
    }

    /**
     * Returns a TmFmt that outputs according to the `asctime` format in ISO
     * C, in the local timezone.
     *
     * Example: "Thu Jan  1 00:00:00 1970"
     */
    pub fn ctime(&self) -> TmFmt {
        TmFmt {
            tm: self,
            format: Fmt::Ctime,
        }
    }

    /**
     * Returns a TmFmt that outputs according to the `asctime` format in ISO
     * C.
     *
     * Example: "Thu Jan  1 00:00:00 1970"
     */
    pub fn asctime(&self) -> TmFmt {
        TmFmt {
            tm: self,
            format: Fmt::Str("%c"),
        }
    }

    /// Formats the time according to the format string.
    pub fn strftime<'a>(&'a self, format: &'a str) -> Result<TmFmt<'a>, ParseError> {
        validate_format(TmFmt {
            tm: self,
            format: Fmt::Str(format),
        })
    }

    /**
     * Returns a TmFmt that outputs according to RFC 822.
     *
     * local: "Thu, 22 Mar 2012 07:53:18 PST"
     * utc:   "Thu, 22 Mar 2012 14:53:18 GMT"
     */
    pub fn rfc822(&self) -> TmFmt {
        let fmt = if self.tm_utcoff == 0 {
            "%a, %d %b %Y %T GMT"
        } else {
            "%a, %d %b %Y %T %Z"
        };
        TmFmt {
            tm: self,
            format: Fmt::Str(fmt),
        }
    }

    /**
     * Returns a TmFmt that outputs according to RFC 822 with Zulu time.
     *
     * local: "Thu, 22 Mar 2012 07:53:18 -0700"
     * utc:   "Thu, 22 Mar 2012 14:53:18 -0000"
     */
    pub fn rfc822z(&self) -> TmFmt {
        TmFmt {
            tm: self,
            format: Fmt::Str("%a, %d %b %Y %T %z"),
        }
    }

    /**
     * Returns a TmFmt that outputs according to RFC 3339. RFC 3339 is
     * compatible with ISO 8601.
     *
     * local: "2012-02-22T07:53:18-07:00"
     * utc:   "2012-02-22T14:53:18Z"
     */
    pub fn rfc3339<'a>(&'a self) -> TmFmt {
        TmFmt {
            tm: self,
            format: Fmt::Rfc3339,
        }
    }
}

#[derive(Copy, PartialEq, Debug, Clone)]
pub enum ParseError {
    InvalidSecond,
    InvalidMinute,
    InvalidHour,
    InvalidDay,
    InvalidMonth,
    InvalidYear,
    InvalidDayOfWeek,
    InvalidDayOfMonth,
    InvalidDayOfYear,
    InvalidZoneOffset,
    InvalidTime,
    InvalidSecondsSinceEpoch,
    MissingFormatConverter,
    InvalidFormatSpecifier(char),
    UnexpectedCharacter(char, char),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        #[allow(deprecated)]
        match *self {
            InvalidFormatSpecifier(ch) => {
                write!(f, "{}: %{}", self.description(), ch)
            }
            UnexpectedCharacter(a, b) => {
                write!(f, "expected: `{}`, found: `{}`", a, b)
            }
            _ => write!(f, "{}", self.description())
        }
    }
}

impl Error for ParseError {
    fn description(&self) -> &str {
        match *self {
            InvalidSecond => "Invalid second.",
            InvalidMinute => "Invalid minute.",
            InvalidHour => "Invalid hour.",
            InvalidDay => "Invalid day.",
            InvalidMonth => "Invalid month.",
            InvalidYear => "Invalid year.",
            InvalidDayOfWeek => "Invalid day of the week.",
            InvalidDayOfMonth => "Invalid day of the month.",
            InvalidDayOfYear => "Invalid day of the year.",
            InvalidZoneOffset => "Invalid zone offset.",
            InvalidTime => "Invalid time.",
            InvalidSecondsSinceEpoch => "Invalid seconds since epoch.",
            MissingFormatConverter => "missing format converter after `%`",
            InvalidFormatSpecifier(..) => "invalid format specifier",
            UnexpectedCharacter(..) => "Unexpected character.",
        }
    }
}

/// A wrapper around a `Tm` and format string that implements Display.
#[derive(Debug)]
pub struct TmFmt<'a> {
    tm: &'a Tm,
    format: Fmt<'a>
}

#[derive(Debug)]
enum Fmt<'a> {
    Str(&'a str),
    Rfc3339,
    Ctime,
}

fn validate_format<'a>(fmt: TmFmt<'a>) -> Result<TmFmt<'a>, ParseError> {

    match (fmt.tm.tm_wday, fmt.tm.tm_mon) {
        (0...6, 0...11) => (),
        (_wday, 0...11) => return Err(InvalidDayOfWeek),
        (0...6, _mon) => return Err(InvalidMonth),
        _ => return Err(InvalidDay)
    }
    match fmt.format {
        Fmt::Str(ref s) => {
            let mut chars = s.chars();
            loop {
                match chars.next() {
                    Some('%') => {
                        match chars.next() {
                            Some('A') | Some('a') | Some('B') | Some('b') |
                            Some('C') | Some('c') | Some('D') | Some('d') |
                            Some('e') | Some('F') | Some('f') | Some('G') |
                            Some('g') | Some('H') | Some('h') | Some('I') |
                            Some('j') | Some('k') | Some('l') | Some('M') |
                            Some('m') | Some('n') | Some('P') | Some('p') |
                            Some('R') | Some('r') | Some('S') | Some('s') |
                            Some('T') | Some('t') | Some('U') | Some('u') |
                            Some('V') | Some('v') | Some('W') | Some('w') |
                            Some('X') | Some('x') | Some('Y') | Some('y') |
                            Some('Z') | Some('z') | Some('+') | Some('%') => (),

                            Some(c) => return Err(InvalidFormatSpecifier(c)),
                            None => return Err(MissingFormatConverter),
                        }
                    },
                    None => break,
                    _ => ()
                }
            }
        },
        _ => ()
    }
    Ok(fmt)
}

/// Formats the time according to the format string.
pub fn strftime(format: &str, tm: &Tm) -> Result<String, ParseError> {
    tm.strftime(format).map(|fmt| fmt.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Timespec, get_time, precise_time_ns, precise_time_s,
                at_utc, at, strptime, PreciseTime, SteadyTime, ParseError, Duration};
    use super::ParseError::{InvalidTime, InvalidYear, MissingFormatConverter,
                            InvalidFormatSpecifier};

    #[allow(deprecated)] // `Once::new` is const starting in Rust 1.32
    use std::sync::ONCE_INIT;
    use std::sync::{Once, Mutex, MutexGuard, LockResult};
    use std::i32;
    use std::mem;

    struct TzReset {
        _tzreset: ::sys::TzReset,
        _lock: LockResult<MutexGuard<'static, ()>>,
    }

    fn set_time_zone_la_or_london(london: bool) -> TzReset {
        // Lock manages current timezone because some tests require LA some
        // London
        static mut LOCK: *mut Mutex<()> = 0 as *mut _;
        #[allow(deprecated)] // `Once::new` is const starting in Rust 1.32
        static INIT: Once = ONCE_INIT;

        unsafe {
            INIT.call_once(|| {
                LOCK = mem::transmute(Box::new(Mutex::new(())));
            });

            let timezone_lock = (*LOCK).lock();
            let reset_func = if london {
                ::sys::set_london_with_dst_time_zone()
            } else {
                ::sys::set_los_angeles_time_zone()
            };
            TzReset {
                _lock: timezone_lock,
                _tzreset: reset_func,
            }
        }
    }

    fn set_time_zone() -> TzReset {
        set_time_zone_la_or_london(false)
    }

    fn set_time_zone_london_dst() -> TzReset {
        set_time_zone_la_or_london(true)
    }

    #[test]
    fn test_get_time() {
        static SOME_RECENT_DATE: i64 = 1577836800i64; // 2020-01-01T00:00:00Z
        static SOME_FUTURE_DATE: i64 = i32::MAX as i64; // Y2038

        let tv1 = get_time();
        debug!("tv1={} sec + {} nsec", tv1.sec, tv1.nsec);

        assert!(tv1.sec > SOME_RECENT_DATE);
        assert!(tv1.nsec < 1000000000i32);

        let tv2 = get_time();
        debug!("tv2={} sec + {} nsec", tv2.sec, tv2.nsec);

        assert!(tv2.sec >= tv1.sec);
        assert!(tv2.sec < SOME_FUTURE_DATE);
        assert!(tv2.nsec < 1000000000i32);
        if tv2.sec == tv1.sec {
            assert!(tv2.nsec >= tv1.nsec);
        }
    }

    #[test]
    fn test_precise_time() {
        let s0 = precise_time_s();
        debug!("s0={} sec", s0);
        assert!(s0 > 0.);

        let ns0 = precise_time_ns();
        let ns1 = precise_time_ns();
        debug!("ns0={} ns", ns0);
        debug!("ns1={} ns", ns1);
        assert!(ns1 >= ns0);

        let ns2 = precise_time_ns();
        debug!("ns2={} ns", ns2);
        assert!(ns2 >= ns1);
    }

    #[test]
    fn test_precise_time_to() {
        let t0 = PreciseTime(1000);
        let t1 = PreciseTime(1023);
        assert_eq!(Duration::nanoseconds(23), t0.to(t1));
    }

    #[test]
    fn test_at_utc() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc = at_utc(time);

        assert_eq!(utc.tm_sec, 30);
        assert_eq!(utc.tm_min, 31);
        assert_eq!(utc.tm_hour, 23);
        assert_eq!(utc.tm_mday, 13);
        assert_eq!(utc.tm_mon, 1);
        assert_eq!(utc.tm_year, 109);
        assert_eq!(utc.tm_wday, 5);
        assert_eq!(utc.tm_yday, 43);
        assert_eq!(utc.tm_isdst, 0);
        assert_eq!(utc.tm_utcoff, 0);
        assert_eq!(utc.tm_nsec, 54321);
    }

    #[test]
    fn test_at() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let local = at(time);

        debug!("time_at: {:?}", local);

        assert_eq!(local.tm_sec, 30);
        assert_eq!(local.tm_min, 31);
        assert_eq!(local.tm_hour, 15);
        assert_eq!(local.tm_mday, 13);
        assert_eq!(local.tm_mon, 1);
        assert_eq!(local.tm_year, 109);
        assert_eq!(local.tm_wday, 5);
        assert_eq!(local.tm_yday, 43);
        assert_eq!(local.tm_isdst, 0);
        assert_eq!(local.tm_utcoff, -28800);
        assert_eq!(local.tm_nsec, 54321);
    }

    #[test]
    fn test_to_timespec() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc = at_utc(time);

        assert_eq!(utc.to_timespec(), time);
        assert_eq!(utc.to_local().to_timespec(), time);
    }

    #[test]
    fn test_conversions() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc = at_utc(time);
        let local = at(time);

        assert!(local.to_local() == local);
        assert!(local.to_utc() == utc);
        assert!(local.to_utc().to_local() == local);
        assert!(utc.to_utc() == utc);
        assert!(utc.to_local() == local);
        assert!(utc.to_local().to_utc() == utc);
    }

    #[test]
    fn test_strptime() {
        let _reset = set_time_zone();

        match strptime("", "") {
            Ok(ref tm) => {
                assert!(tm.tm_sec == 0);
                assert!(tm.tm_min == 0);
                assert!(tm.tm_hour == 0);
                assert!(tm.tm_mday == 0);
                assert!(tm.tm_mon == 0);
                assert!(tm.tm_year == 0);
                assert!(tm.tm_wday == 0);
                assert!(tm.tm_isdst == 0);
                assert!(tm.tm_utcoff == 0);
                assert!(tm.tm_nsec == 0);
            }
            Err(_) => ()
        }

        let format = "%a %b %e %T.%f %Y";
        assert_eq!(strptime("", format), Err(ParseError::InvalidDay));
        assert_eq!(strptime("Fri Feb 13 15:31:30", format),
                   Err(InvalidTime));

        match strptime("Fri Feb 13 15:31:30.01234 2009", format) {
            Err(e) => panic!("{}", e),
            Ok(ref tm) => {
                assert_eq!(tm.tm_sec, 30);
                assert_eq!(tm.tm_min, 31);
                assert_eq!(tm.tm_hour, 15);
                assert_eq!(tm.tm_mday, 13);
                assert_eq!(tm.tm_mon, 1);
                assert_eq!(tm.tm_year, 109);
                assert_eq!(tm.tm_wday, 5);
                assert_eq!(tm.tm_yday, 0);
                assert_eq!(tm.tm_isdst, 0);
                assert_eq!(tm.tm_utcoff, 0);
                assert_eq!(tm.tm_nsec, 12340000);
            }
        }

        fn test(s: &str, format: &str) -> bool {
            match strptime(s, format) {
              Ok(tm) => {
                tm.strftime(format).unwrap().to_string() == s.to_string()
              },
              Err(e) => panic!("{:?},  s={:?}, format={:?}", e, s, format)
            }
        }

        fn test_oneway(s : &str, format : &str) -> bool {
            match strptime(s, format) {
              Ok(_) => {
                // oneway tests are used when reformatting the parsed Tm
                // back into a string can generate a different string
                // from the original (i.e. leading zeroes)
                true
              },
              Err(e) => panic!("{:?},  s={:?}, format={:?}", e, s, format)
            }
        }

        let days = [
            "Sunday".to_string(),
            "Monday".to_string(),
            "Tuesday".to_string(),
            "Wednesday".to_string(),
            "Thursday".to_string(),
            "Friday".to_string(),
            "Saturday".to_string()
        ];
        for day in days.iter() {
            assert!(test(&day, "%A"));
        }

        let days = [
            "Sun".to_string(),
            "Mon".to_string(),
            "Tue".to_string(),
            "Wed".to_string(),
            "Thu".to_string(),
            "Fri".to_string(),
            "Sat".to_string()
        ];
        for day in days.iter() {
            assert!(test(&day, "%a"));
        }

        let months = [
            "January".to_string(),
            "February".to_string(),
            "March".to_string(),
            "April".to_string(),
            "May".to_string(),
            "June".to_string(),
            "July".to_string(),
            "August".to_string(),
            "September".to_string(),
            "October".to_string(),
            "November".to_string(),
            "December".to_string()
        ];
        for day in months.iter() {
            assert!(test(&day, "%B"));
        }

        let months = [
            "Jan".to_string(),
            "Feb".to_string(),
            "Mar".to_string(),
            "Apr".to_string(),
            "May".to_string(),
            "Jun".to_string(),
            "Jul".to_string(),
            "Aug".to_string(),
            "Sep".to_string(),
            "Oct".to_string(),
            "Nov".to_string(),
            "Dec".to_string()
        ];
        for day in months.iter() {
            assert!(test(&day, "%b"));
        }

        assert!(test("19", "%C"));
        assert!(test("Fri Feb  3 23:31:30 2009", "%c"));
        assert!(test("Fri Feb 13 23:31:30 2009", "%c"));
        assert!(test("02/13/09", "%D"));
        assert!(test("03", "%d"));
        assert!(test("13", "%d"));
        assert!(test(" 3", "%e"));
        assert!(test("13", "%e"));
        assert!(test("2009-02-13", "%F"));
        assert!(test("03", "%H"));
        assert!(test("13", "%H"));
        assert!(test("03", "%I")); // FIXME (#2350): flesh out
        assert!(test("11", "%I")); // FIXME (#2350): flesh out
        assert!(test("044", "%j"));
        assert!(test(" 3", "%k"));
        assert!(test("13", "%k"));
        assert!(test(" 1", "%l"));
        assert!(test("11", "%l"));
        assert!(test("03", "%M"));
        assert!(test("13", "%M"));
        assert!(test("\n", "%n"));
        assert!(test("am", "%P"));
        assert!(test("pm", "%P"));
        assert!(test("AM", "%p"));
        assert!(test("PM", "%p"));
        assert!(test("23:31", "%R"));
        assert!(test("11:31:30 AM", "%r"));
        assert!(test("11:31:30 PM", "%r"));
        assert!(test("03", "%S"));
        assert!(test("13", "%S"));
        assert!(test("15:31:30", "%T"));
        assert!(test("\t", "%t"));
        assert!(test("1", "%u"));
        assert!(test("7", "%u"));
        assert!(test("13-Feb-2009", "%v"));
        assert!(test("0", "%w"));
        assert!(test("6", "%w"));
        assert!(test("2009", "%Y"));
        assert!(test("09", "%y"));

        assert!(test_oneway("3",  "%d"));
        assert!(test_oneway("3",  "%H"));
        assert!(test_oneway("3",  "%e"));
        assert!(test_oneway("3",  "%M"));
        assert!(test_oneway("3",  "%S"));

        assert!(strptime("-0000", "%z").unwrap().tm_utcoff == 0);
        assert!(strptime("-00:00", "%z").unwrap().tm_utcoff == 0);
        assert!(strptime("Z", "%z").unwrap().tm_utcoff == 0);
        assert_eq!(-28800, strptime("-0800", "%z").unwrap().tm_utcoff);
        assert_eq!(-28800, strptime("-08:00", "%z").unwrap().tm_utcoff);
        assert_eq!(28800, strptime("+0800", "%z").unwrap().tm_utcoff);
        assert_eq!(28800, strptime("+08:00", "%z").unwrap().tm_utcoff);
        assert_eq!(5400, strptime("+0130", "%z").unwrap().tm_utcoff);
        assert_eq!(5400, strptime("+01:30", "%z").unwrap().tm_utcoff);
        assert!(test("%", "%%"));

        // Test for #7256
        assert_eq!(strptime("360", "%Y-%m-%d"), Err(InvalidYear));

        // Test for epoch seconds parsing
        {
            assert!(test("1428035610", "%s"));
            let tm = strptime("1428035610", "%s").unwrap();
            assert_eq!(tm.tm_utcoff, 0);
            assert_eq!(tm.tm_isdst, 0);
            assert_eq!(tm.tm_yday, 92);
            assert_eq!(tm.tm_wday, 5);
            assert_eq!(tm.tm_year, 115);
            assert_eq!(tm.tm_mon, 3);
            assert_eq!(tm.tm_mday, 3);
            assert_eq!(tm.tm_hour, 4);
        }
    }

    #[test]
    fn test_asctime() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc   = at_utc(time);
        let local = at(time);

        debug!("test_ctime: {} {}", utc.asctime(), local.asctime());

        assert_eq!(utc.asctime().to_string(), "Fri Feb 13 23:31:30 2009".to_string());
        assert_eq!(local.asctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
    }

    #[test]
    fn test_ctime() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc   = at_utc(time);
        let local = at(time);

        debug!("test_ctime: {} {}", utc.ctime(), local.ctime());

        assert_eq!(utc.ctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
        assert_eq!(local.ctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
    }

    #[test]
    fn test_strftime() {
        let _reset = set_time_zone();

        let time = Timespec::new(1234567890, 54321);
        let utc = at_utc(time);
        let local = at(time);

        assert_eq!(local.strftime("").unwrap().to_string(), "".to_string());
        assert_eq!(local.strftime("%A").unwrap().to_string(), "Friday".to_string());
        assert_eq!(local.strftime("%a").unwrap().to_string(), "Fri".to_string());
        assert_eq!(local.strftime("%B").unwrap().to_string(), "February".to_string());
        assert_eq!(local.strftime("%b").unwrap().to_string(), "Feb".to_string());
        assert_eq!(local.strftime("%C").unwrap().to_string(), "20".to_string());
        assert_eq!(local.strftime("%c").unwrap().to_string(),
                   "Fri Feb 13 15:31:30 2009".to_string());
        assert_eq!(local.strftime("%D").unwrap().to_string(), "02/13/09".to_string());
        assert_eq!(local.strftime("%d").unwrap().to_string(), "13".to_string());
        assert_eq!(local.strftime("%e").unwrap().to_string(), "13".to_string());
        assert_eq!(local.strftime("%F").unwrap().to_string(), "2009-02-13".to_string());
        assert_eq!(local.strftime("%f").unwrap().to_string(), "000054321".to_string());
        assert_eq!(local.strftime("%G").unwrap().to_string(), "2009".to_string());
        assert_eq!(local.strftime("%g").unwrap().to_string(), "09".to_string());
        assert_eq!(local.strftime("%H").unwrap().to_string(), "15".to_string());
        assert_eq!(local.strftime("%h").unwrap().to_string(), "Feb".to_string());
        assert_eq!(local.strftime("%I").unwrap().to_string(), "03".to_string());
        assert_eq!(local.strftime("%j").unwrap().to_string(), "044".to_string());
        assert_eq!(local.strftime("%k").unwrap().to_string(), "15".to_string());
        assert_eq!(local.strftime("%l").unwrap().to_string(), " 3".to_string());
        assert_eq!(local.strftime("%M").unwrap().to_string(), "31".to_string());
        assert_eq!(local.strftime("%m").unwrap().to_string(), "02".to_string());
        assert_eq!(local.strftime("%n").unwrap().to_string(), "\n".to_string());
        assert_eq!(local.strftime("%P").unwrap().to_string(), "pm".to_string());
        assert_eq!(local.strftime("%p").unwrap().to_string(), "PM".to_string());
        assert_eq!(local.strftime("%R").unwrap().to_string(), "15:31".to_string());
        assert_eq!(local.strftime("%r").unwrap().to_string(), "03:31:30 PM".to_string());
        assert_eq!(local.strftime("%S").unwrap().to_string(), "30".to_string());
        assert_eq!(local.strftime("%s").unwrap().to_string(), "1234567890".to_string());
        assert_eq!(local.strftime("%T").unwrap().to_string(), "15:31:30".to_string());
        assert_eq!(local.strftime("%t").unwrap().to_string(), "\t".to_string());
        assert_eq!(local.strftime("%U").unwrap().to_string(), "06".to_string());
        assert_eq!(local.strftime("%u").unwrap().to_string(), "5".to_string());
        assert_eq!(local.strftime("%V").unwrap().to_string(), "07".to_string());
        assert_eq!(local.strftime("%v").unwrap().to_string(), "13-Feb-2009".to_string());
        assert_eq!(local.strftime("%W").unwrap().to_string(), "06".to_string());
        assert_eq!(local.strftime("%w").unwrap().to_string(), "5".to_string());
        // FIXME (#2350): support locale
        assert_eq!(local.strftime("%X").unwrap().to_string(), "15:31:30".to_string());
        // FIXME (#2350): support locale
        assert_eq!(local.strftime("%x").unwrap().to_string(), "02/13/09".to_string());
        assert_eq!(local.strftime("%Y").unwrap().to_string(), "2009".to_string());
        assert_eq!(local.strftime("%y").unwrap().to_string(), "09".to_string());
        // FIXME (#2350): support locale
        assert_eq!(local.strftime("%Z").unwrap().to_string(), "".to_string());
        assert_eq!(local.strftime("%z").unwrap().to_string(), "-0800".to_string());
        assert_eq!(local.strftime("%+").unwrap().to_string(),
                   "2009-02-13T15:31:30-08:00".to_string());
        assert_eq!(local.strftime("%%").unwrap().to_string(), "%".to_string());

         let invalid_specifiers = ["%E", "%J", "%K", "%L", "%N", "%O", "%o", "%Q", "%q"];
        for &sp in invalid_specifiers.iter() {
            assert_eq!(local.strftime(sp).unwrap_err(),
                       InvalidFormatSpecifier(sp[1..].chars().next().unwrap()));
        }
        assert_eq!(local.strftime("%").unwrap_err(), MissingFormatConverter);
        assert_eq!(local.strftime("%A %").unwrap_err(), MissingFormatConverter);

        assert_eq!(local.asctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
        assert_eq!(local.ctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
        assert_eq!(local.rfc822z().to_string(), "Fri, 13 Feb 2009 15:31:30 -0800".to_string());
        assert_eq!(local.rfc3339().to_string(), "2009-02-13T15:31:30-08:00".to_string());

        assert_eq!(utc.asctime().to_string(), "Fri Feb 13 23:31:30 2009".to_string());
        assert_eq!(utc.ctime().to_string(), "Fri Feb 13 15:31:30 2009".to_string());
        assert_eq!(utc.rfc822().to_string(), "Fri, 13 Feb 2009 23:31:30 GMT".to_string());
        assert_eq!(utc.rfc822z().to_string(), "Fri, 13 Feb 2009 23:31:30 -0000".to_string());
        assert_eq!(utc.rfc3339().to_string(), "2009-02-13T23:31:30Z".to_string());
    }

    #[test]
    fn test_timespec_eq_ord() {
        let a = &Timespec::new(-2, 1);
        let b = &Timespec::new(-1, 2);
        let c = &Timespec::new(1, 2);
        let d = &Timespec::new(2, 1);
        let e = &Timespec::new(2, 1);

        assert!(d.eq(e));
        assert!(c.ne(e));

        assert!(a.lt(b));
        assert!(b.lt(c));
        assert!(c.lt(d));

        assert!(a.le(b));
        assert!(b.le(c));
        assert!(c.le(d));
        assert!(d.le(e));
        assert!(e.le(d));

        assert!(b.ge(a));
        assert!(c.ge(b));
        assert!(d.ge(c));
        assert!(e.ge(d));
        assert!(d.ge(e));

        assert!(b.gt(a));
        assert!(c.gt(b));
        assert!(d.gt(c));
    }

    #[test]
    #[allow(deprecated)]
    fn test_timespec_hash() {
        use std::hash::{Hash, Hasher};

        let c = &Timespec::new(3, 2);
        let d = &Timespec::new(2, 1);
        let e = &Timespec::new(2, 1);

        let mut hasher = ::std::hash::SipHasher::new();

        let d_hash:u64 = {
          d.hash(&mut hasher);
          hasher.finish()
        };

        hasher = ::std::hash::SipHasher::new();

        let e_hash:u64 = {
          e.hash(&mut hasher);
          hasher.finish()
        };

        hasher = ::std::hash::SipHasher::new();

        let c_hash:u64 = {
          c.hash(&mut hasher);
          hasher.finish()
        };

        assert_eq!(d_hash, e_hash);
        assert!(c_hash != e_hash);
    }

    #[test]
    fn test_timespec_add() {
        let a = Timespec::new(1, 2);
        let b = Duration::seconds(2) + Duration::nanoseconds(3);
        let c = a + b;
        assert_eq!(c.sec, 3);
        assert_eq!(c.nsec, 5);

        let p = Timespec::new(1, super::NSEC_PER_SEC - 2);
        let q = Duration::seconds(2) + Duration::nanoseconds(2);
        let r = p + q;
        assert_eq!(r.sec, 4);
        assert_eq!(r.nsec, 0);

        let u = Timespec::new(1, super::NSEC_PER_SEC - 2);
        let v = Duration::seconds(2) + Duration::nanoseconds(3);
        let w = u + v;
        assert_eq!(w.sec, 4);
        assert_eq!(w.nsec, 1);

        let k = Timespec::new(1, 0);
        let l = Duration::nanoseconds(-1);
        let m = k + l;
        assert_eq!(m.sec, 0);
        assert_eq!(m.nsec, 999_999_999);
    }

    #[test]
    fn test_timespec_sub() {
        let a = Timespec::new(2, 3);
        let b = Timespec::new(1, 2);
        let c = a - b;
        assert_eq!(c.num_nanoseconds(), Some(super::NSEC_PER_SEC as i64 + 1));

        let p = Timespec::new(2, 0);
        let q = Timespec::new(1, 2);
        let r = p - q;
        assert_eq!(r.num_nanoseconds(), Some(super::NSEC_PER_SEC as i64 - 2));

        let u = Timespec::new(1, 2);
        let v = Timespec::new(2, 3);
        let w = u - v;
        assert_eq!(w.num_nanoseconds(), Some(-super::NSEC_PER_SEC as i64 - 1));
    }

    #[test]
    fn test_time_sub() {
        let a = ::now();
        let b = at(a.to_timespec() + Duration::seconds(5));
        let c = b - a;
        assert_eq!(c.num_nanoseconds(), Some(super::NSEC_PER_SEC as i64 * 5));
    }

    #[test]
    fn test_steadytime_sub() {
        let a = SteadyTime::now();
        let b = a + Duration::seconds(1);
        assert_eq!(b - a, Duration::seconds(1));
        assert_eq!(a - b, Duration::seconds(-1));
    }

    #[test]
    fn test_date_before_1970() {
        let early = strptime("1901-01-06", "%F").unwrap();
        let late = strptime("2000-01-01", "%F").unwrap();
        assert!(early < late);
    }

    #[test]
    fn test_dst() {
        let _reset = set_time_zone_london_dst();
        let utc_in_feb = strptime("2015-02-01Z", "%F%z").unwrap();
        let utc_in_jun = strptime("2015-06-01Z", "%F%z").unwrap();
        let utc_in_nov = strptime("2015-11-01Z", "%F%z").unwrap();
        let local_in_feb = utc_in_feb.to_local();
        let local_in_jun = utc_in_jun.to_local();
        let local_in_nov = utc_in_nov.to_local();

        assert_eq!(local_in_feb.tm_mon, 1);
        assert_eq!(local_in_feb.tm_hour, 0);
        assert_eq!(local_in_feb.tm_utcoff, 0);
        assert_eq!(local_in_feb.tm_isdst, 0);

        assert_eq!(local_in_jun.tm_mon, 5);
        assert_eq!(local_in_jun.tm_hour, 1);
        assert_eq!(local_in_jun.tm_utcoff, 3600);
        assert_eq!(local_in_jun.tm_isdst, 1);

        assert_eq!(local_in_nov.tm_mon, 10);
        assert_eq!(local_in_nov.tm_hour, 0);
        assert_eq!(local_in_nov.tm_utcoff, 0);
        assert_eq!(local_in_nov.tm_isdst, 0)
    }
}
