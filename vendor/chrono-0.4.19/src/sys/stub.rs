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

fn time_to_tm(ts: i64, tm: &mut Tm) {
    let leapyear = |year| -> bool { year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) };

    static YTAB: [[i64; 12]; 2] = [
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    ];

    let mut year = 1970;

    let dayclock = ts % 86400;
    let mut dayno = ts / 86400;

    tm.tm_sec = (dayclock % 60) as i32;
    tm.tm_min = ((dayclock % 3600) / 60) as i32;
    tm.tm_hour = (dayclock / 3600) as i32;
    tm.tm_wday = ((dayno + 4) % 7) as i32;
    loop {
        let yearsize = if leapyear(year) { 366 } else { 365 };
        if dayno >= yearsize {
            dayno -= yearsize;
            year += 1;
        } else {
            break;
        }
    }
    tm.tm_year = (year - 1900) as i32;
    tm.tm_yday = dayno as i32;
    let mut mon = 0;
    while dayno >= YTAB[if leapyear(year) { 1 } else { 0 }][mon] {
        dayno -= YTAB[if leapyear(year) { 1 } else { 0 }][mon];
        mon += 1;
    }
    tm.tm_mon = mon as i32;
    tm.tm_mday = dayno as i32 + 1;
    tm.tm_isdst = 0;
}

fn tm_to_time(tm: &Tm) -> i64 {
    let mut y = tm.tm_year as i64 + 1900;
    let mut m = tm.tm_mon as i64 + 1;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let d = tm.tm_mday as i64;
    let h = tm.tm_hour as i64;
    let mi = tm.tm_min as i64;
    let s = tm.tm_sec as i64;
    (365 * y + y / 4 - y / 100 + y / 400 + 3 * (m + 1) / 5 + 30 * m + d - 719561) * 86400
        + 3600 * h
        + 60 * mi
        + s
}

pub fn time_to_local_tm(sec: i64, tm: &mut Tm) {
    // FIXME: Add timezone logic
    time_to_tm(sec, tm);
}

pub fn utc_tm_to_time(tm: &Tm) -> i64 {
    tm_to_time(tm)
}

pub fn local_tm_to_time(tm: &Tm) -> i64 {
    // FIXME: Add timezone logic
    tm_to_time(tm)
}
