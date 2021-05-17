// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::Tm;
use libc::{self, time_t};
use std::io;
use std::mem;

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
extern "C" {
    static timezone: time_t;
    static altzone: time_t;
}

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
fn tzset() {
    extern "C" {
        fn tzset();
    }
    unsafe { tzset() }
}

fn rust_tm_to_tm(rust_tm: &Tm, tm: &mut libc::tm) {
    tm.tm_sec = rust_tm.tm_sec;
    tm.tm_min = rust_tm.tm_min;
    tm.tm_hour = rust_tm.tm_hour;
    tm.tm_mday = rust_tm.tm_mday;
    tm.tm_mon = rust_tm.tm_mon;
    tm.tm_year = rust_tm.tm_year;
    tm.tm_wday = rust_tm.tm_wday;
    tm.tm_yday = rust_tm.tm_yday;
    tm.tm_isdst = rust_tm.tm_isdst;
}

fn tm_to_rust_tm(tm: &libc::tm, utcoff: i32, rust_tm: &mut Tm) {
    rust_tm.tm_sec = tm.tm_sec;
    rust_tm.tm_min = tm.tm_min;
    rust_tm.tm_hour = tm.tm_hour;
    rust_tm.tm_mday = tm.tm_mday;
    rust_tm.tm_mon = tm.tm_mon;
    rust_tm.tm_year = tm.tm_year;
    rust_tm.tm_wday = tm.tm_wday;
    rust_tm.tm_yday = tm.tm_yday;
    rust_tm.tm_isdst = tm.tm_isdst;
    rust_tm.tm_utcoff = utcoff;
}

#[cfg(any(target_os = "nacl", target_os = "solaris", target_os = "illumos"))]
unsafe fn timegm(tm: *mut libc::tm) -> time_t {
    use std::env::{remove_var, set_var, var_os};
    extern "C" {
        fn tzset();
    }

    let ret;

    let current_tz = var_os("TZ");
    set_var("TZ", "UTC");
    tzset();

    ret = libc::mktime(tm);

    if let Some(tz) = current_tz {
        set_var("TZ", tz);
    } else {
        remove_var("TZ");
    }
    tzset();

    ret
}

pub fn time_to_local_tm(sec: i64, tm: &mut Tm) {
    unsafe {
        let sec = sec as time_t;
        let mut out = mem::zeroed();
        if libc::localtime_r(&sec, &mut out).is_null() {
            panic!("localtime_r failed: {}", io::Error::last_os_error());
        }
        #[cfg(any(target_os = "solaris", target_os = "illumos"))]
        let gmtoff = {
            tzset();
            // < 0 means we don't know; assume we're not in DST.
            if out.tm_isdst == 0 {
                // timezone is seconds west of UTC, tm_gmtoff is seconds east
                -timezone
            } else if out.tm_isdst > 0 {
                -altzone
            } else {
                -timezone
            }
        };
        #[cfg(not(any(target_os = "solaris", target_os = "illumos")))]
        let gmtoff = out.tm_gmtoff;
        tm_to_rust_tm(&out, gmtoff as i32, tm);
    }
}

pub fn utc_tm_to_time(rust_tm: &Tm) -> i64 {
    #[cfg(not(any(
        all(target_os = "android", target_pointer_width = "32"),
        target_os = "nacl",
        target_os = "solaris",
        target_os = "illumos"
    )))]
    use libc::timegm;
    #[cfg(all(target_os = "android", target_pointer_width = "32"))]
    use libc::timegm64 as timegm;

    let mut tm = unsafe { mem::zeroed() };
    rust_tm_to_tm(rust_tm, &mut tm);
    unsafe { timegm(&mut tm) as i64 }
}

pub fn local_tm_to_time(rust_tm: &Tm) -> i64 {
    let mut tm = unsafe { mem::zeroed() };
    rust_tm_to_tm(rust_tm, &mut tm);
    unsafe { libc::mktime(&mut tm) as i64 }
}
