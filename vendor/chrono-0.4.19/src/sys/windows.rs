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
use std::io;
use std::mem;

use winapi::shared::minwindef::*;
use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::timezoneapi::*;

const HECTONANOSECS_IN_SEC: i64 = 10_000_000;
const HECTONANOSEC_TO_UNIX_EPOCH: i64 = 11_644_473_600 * HECTONANOSECS_IN_SEC;

fn time_to_file_time(sec: i64) -> FILETIME {
    let t = ((sec * HECTONANOSECS_IN_SEC) + HECTONANOSEC_TO_UNIX_EPOCH) as u64;
    FILETIME { dwLowDateTime: t as DWORD, dwHighDateTime: (t >> 32) as DWORD }
}

fn file_time_as_u64(ft: &FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn file_time_to_unix_seconds(ft: &FILETIME) -> i64 {
    let t = file_time_as_u64(ft) as i64;
    ((t - HECTONANOSEC_TO_UNIX_EPOCH) / HECTONANOSECS_IN_SEC) as i64
}

fn system_time_to_file_time(sys: &SYSTEMTIME) -> FILETIME {
    unsafe {
        let mut ft = mem::zeroed();
        SystemTimeToFileTime(sys, &mut ft);
        ft
    }
}

fn tm_to_system_time(tm: &Tm) -> SYSTEMTIME {
    let mut sys: SYSTEMTIME = unsafe { mem::zeroed() };
    sys.wSecond = tm.tm_sec as WORD;
    sys.wMinute = tm.tm_min as WORD;
    sys.wHour = tm.tm_hour as WORD;
    sys.wDay = tm.tm_mday as WORD;
    sys.wDayOfWeek = tm.tm_wday as WORD;
    sys.wMonth = (tm.tm_mon + 1) as WORD;
    sys.wYear = (tm.tm_year + 1900) as WORD;
    sys
}

fn system_time_to_tm(sys: &SYSTEMTIME, tm: &mut Tm) {
    tm.tm_sec = sys.wSecond as i32;
    tm.tm_min = sys.wMinute as i32;
    tm.tm_hour = sys.wHour as i32;
    tm.tm_mday = sys.wDay as i32;
    tm.tm_wday = sys.wDayOfWeek as i32;
    tm.tm_mon = (sys.wMonth - 1) as i32;
    tm.tm_year = (sys.wYear - 1900) as i32;
    tm.tm_yday = yday(tm.tm_year, tm.tm_mon + 1, tm.tm_mday);

    fn yday(year: i32, month: i32, day: i32) -> i32 {
        let leap = if month > 2 {
            if year % 4 == 0 {
                1
            } else {
                2
            }
        } else {
            0
        };
        let july = if month > 7 { 1 } else { 0 };

        (month - 1) * 30 + month / 2 + (day - 1) - leap + july
    }
}

macro_rules! call {
    ($name:ident($($arg:expr),*)) => {
        if $name($($arg),*) == 0 {
            panic!(concat!(stringify!($name), " failed with: {}"),
                    io::Error::last_os_error());
        }
    }
}

pub fn time_to_local_tm(sec: i64, tm: &mut Tm) {
    let ft = time_to_file_time(sec);
    unsafe {
        let mut utc = mem::zeroed();
        let mut local = mem::zeroed();
        call!(FileTimeToSystemTime(&ft, &mut utc));
        call!(SystemTimeToTzSpecificLocalTime(0 as *const _, &mut utc, &mut local));
        system_time_to_tm(&local, tm);

        let local = system_time_to_file_time(&local);
        let local_sec = file_time_to_unix_seconds(&local);

        let mut tz = mem::zeroed();
        GetTimeZoneInformation(&mut tz);

        // SystemTimeToTzSpecificLocalTime already applied the biases so
        // check if it non standard
        tm.tm_utcoff = (local_sec - sec) as i32;
        tm.tm_isdst = if tm.tm_utcoff == -60 * (tz.Bias + tz.StandardBias) { 0 } else { 1 };
    }
}

pub fn utc_tm_to_time(tm: &Tm) -> i64 {
    unsafe {
        let mut ft = mem::zeroed();
        let sys_time = tm_to_system_time(tm);
        call!(SystemTimeToFileTime(&sys_time, &mut ft));
        file_time_to_unix_seconds(&ft)
    }
}

pub fn local_tm_to_time(tm: &Tm) -> i64 {
    unsafe {
        let mut ft = mem::zeroed();
        let mut utc = mem::zeroed();
        let mut sys_time = tm_to_system_time(tm);
        call!(TzSpecificLocalTimeToSystemTime(0 as *mut _, &mut sys_time, &mut utc));
        call!(SystemTimeToFileTime(&utc, &mut ft));
        file_time_to_unix_seconds(&ft)
    }
}
