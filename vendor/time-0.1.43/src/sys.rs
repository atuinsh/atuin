#![allow(bad_style)]

pub use self::inner::*;

#[cfg(any(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    target_env = "sgx"
))]
mod common {
    use Tm;

    pub fn time_to_tm(ts: i64, tm: &mut Tm) {
        let leapyear = |year| -> bool {
            year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
        };

        static _ytab: [[i64; 12]; 2] = [
            [ 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 ],
            [ 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 ]
        ];

        let mut year = 1970;

        let dayclock = ts % 86400;
        let mut dayno = ts / 86400;

        tm.tm_sec = (dayclock % 60) as i32;
        tm.tm_min = ((dayclock % 3600) / 60) as i32;
        tm.tm_hour = (dayclock / 3600) as i32;
        tm.tm_wday = ((dayno + 4) % 7) as i32;
        loop {
            let yearsize = if leapyear(year) {
                366
            } else {
                365
            };
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
        while dayno >= _ytab[if leapyear(year) { 1 } else { 0 }][mon] {
                dayno -= _ytab[if leapyear(year) { 1 } else { 0 }][mon];
                mon += 1;
        }
        tm.tm_mon = mon as i32;
        tm.tm_mday = dayno as i32 + 1;
        tm.tm_isdst = 0;
    }

    pub fn tm_to_time(tm: &Tm) -> i64 {
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
        (365*y + y/4 - y/100 + y/400 + 3*(m+1)/5 + 30*m + d - 719561)
            * 86400 + 3600 * h + 60 * mi + s
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]
mod inner {
    use std::ops::{Add, Sub};
    use Tm;
    use Duration;
    use super::common::{time_to_tm, tm_to_time};

    #[derive(Copy, Clone, Debug, PartialOrd, Ord, PartialEq, Eq)]
    pub struct SteadyTime;

    pub fn time_to_utc_tm(sec: i64, tm: &mut Tm) {
        time_to_tm(sec, tm);
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

    pub fn get_time() -> (i64, i32) {
        unimplemented!()
    }

    pub fn get_precise_ns() -> u64 {
        unimplemented!()
    }

    impl SteadyTime {
        pub fn now() -> SteadyTime {
            unimplemented!()
        }
    }

    impl Sub for SteadyTime {
        type Output = Duration;
        fn sub(self, _other: SteadyTime) -> Duration {
            unimplemented!()
        }
    }

    impl Sub<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn sub(self, _other: Duration) -> SteadyTime {
            unimplemented!()
        }
    }

    impl Add<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn add(self, _other: Duration) -> SteadyTime {
            unimplemented!()
        }
    }
}

#[cfg(target_env = "sgx")]
mod inner {
    use std::ops::{Add, Sub};
    use Tm;
    use Duration;
    use super::common::{time_to_tm, tm_to_time};
    use std::time::SystemTime;

    /// The number of nanoseconds in seconds.
    const NANOS_PER_SEC: u64 = 1_000_000_000;

    #[derive(Copy, Clone, Debug, PartialOrd, Ord, PartialEq, Eq)]
    pub struct SteadyTime {
        t: Duration
    }

    pub fn time_to_utc_tm(sec: i64, tm: &mut Tm) {
        time_to_tm(sec, tm);
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

    pub fn get_time() -> (i64, i32) {
        SteadyTime::now().t.raw()
    }

    pub fn get_precise_ns() -> u64 {
        // This unwrap is safe because current time is well ahead of UNIX_EPOCH, unless system
        // clock is adjusted backward.
        let std_duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        std_duration.as_secs() * NANOS_PER_SEC + std_duration.subsec_nanos() as u64
    }

    impl SteadyTime {
        pub fn now() -> SteadyTime {
            // This unwrap is safe because current time is well ahead of UNIX_EPOCH, unless system
            // clock is adjusted backward.
            let std_duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
            // This unwrap is safe because duration is well within the limits of i64.
            let duration = Duration::from_std(std_duration).unwrap();
            SteadyTime { t: duration }
        }
    }

    impl Sub for SteadyTime {
        type Output = Duration;
        fn sub(self, other: SteadyTime) -> Duration {
            self.t - other.t
        }
    }

    impl Sub<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn sub(self, other: Duration) -> SteadyTime {
            SteadyTime { t: self.t - other }
        }
    }

    impl Add<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn add(self, other: Duration) -> SteadyTime {
            SteadyTime { t: self.t + other }
        }
    }
}

#[cfg(unix)]
mod inner {
    use libc::{self, time_t};
    use std::mem;
    use std::io;
    use Tm;

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub use self::mac::*;
    #[cfg(all(not(target_os = "macos"), not(target_os = "ios")))]
    pub use self::unix::*;

    #[cfg(any(target_os = "solaris", target_os = "illumos"))]
    extern {
        static timezone: time_t;
        static altzone: time_t;
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
        use std::env::{set_var, var_os, remove_var};
        extern {
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

    pub fn time_to_utc_tm(sec: i64, tm: &mut Tm) {
        unsafe {
            let sec = sec as time_t;
            let mut out = mem::zeroed();
            if libc::gmtime_r(&sec, &mut out).is_null() {
                panic!("gmtime_r failed: {}", io::Error::last_os_error());
            }
            tm_to_rust_tm(&out, 0, tm);
        }
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
                ::tzset();
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
        #[cfg(all(target_os = "android", target_pointer_width = "32"))]
        use libc::timegm64 as timegm;
        #[cfg(not(any(
            all(target_os = "android", target_pointer_width = "32"),
            target_os = "nacl",
            target_os = "solaris",
            target_os = "illumos"
        )))]
        use libc::timegm;

        let mut tm = unsafe { mem::zeroed() };
        rust_tm_to_tm(rust_tm, &mut tm);
        unsafe { timegm(&mut tm) as i64 }
    }

    pub fn local_tm_to_time(rust_tm: &Tm) -> i64 {
        let mut tm = unsafe { mem::zeroed() };
        rust_tm_to_tm(rust_tm, &mut tm);
        unsafe { libc::mktime(&mut tm) as i64 }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    mod mac {
        #[allow(deprecated)]
        use libc::{self, timeval, mach_timebase_info};
        #[allow(deprecated)]
        use std::sync::{Once, ONCE_INIT};
        use std::ops::{Add, Sub};
        use Duration;

        #[allow(deprecated)]
        fn info() -> &'static mach_timebase_info {
            static mut INFO: mach_timebase_info = mach_timebase_info {
                numer: 0,
                denom: 0,
            };
            static ONCE: Once = ONCE_INIT;

            unsafe {
                ONCE.call_once(|| {
                    mach_timebase_info(&mut INFO);
                });
                &INFO
            }
        }

        pub fn get_time() -> (i64, i32) {
            use std::ptr;
            let mut tv = timeval { tv_sec: 0, tv_usec: 0 };
            unsafe { libc::gettimeofday(&mut tv, ptr::null_mut()); }
            (tv.tv_sec as i64, tv.tv_usec * 1000)
        }

        #[allow(deprecated)]
        #[inline]
        pub fn get_precise_ns() -> u64 {
            unsafe {
                let time = libc::mach_absolute_time();
                let info = info();
                time * info.numer as u64 / info.denom as u64
            }
        }

        #[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug)]
        pub struct SteadyTime { t: u64 }

        impl SteadyTime {
            pub fn now() -> SteadyTime {
                SteadyTime { t: get_precise_ns() }
            }
        }
        impl Sub for SteadyTime {
            type Output = Duration;
            fn sub(self, other: SteadyTime) -> Duration {
                Duration::nanoseconds(self.t as i64 - other.t as i64)
            }
        }
        impl Sub<Duration> for SteadyTime {
            type Output = SteadyTime;
            fn sub(self, other: Duration) -> SteadyTime {
                self + -other
            }
        }
        impl Add<Duration> for SteadyTime {
            type Output = SteadyTime;
            fn add(self, other: Duration) -> SteadyTime {
                let delta = other.num_nanoseconds().unwrap();
                SteadyTime {
                    t: (self.t as i64 + delta) as u64
                }
            }
        }
    }

    #[cfg(test)]
    pub struct TzReset;

    #[cfg(test)]
    pub fn set_los_angeles_time_zone() -> TzReset {
        use std::env;
        env::set_var("TZ", "America/Los_Angeles");
        ::tzset();
        TzReset
    }

    #[cfg(test)]
    pub fn set_london_with_dst_time_zone() -> TzReset {
        use std::env;
        env::set_var("TZ", "Europe/London");
        ::tzset();
        TzReset
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "ios")))]
    mod unix {
        use std::fmt;
        use std::cmp::Ordering;
        use std::ops::{Add, Sub};
        use libc;

        use Duration;

        pub fn get_time() -> (i64, i32) {
            let mut tv = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut tv); }
            (tv.tv_sec as i64, tv.tv_nsec as i32)
        }

        pub fn get_precise_ns() -> u64 {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            unsafe {
                libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
            }
            (ts.tv_sec as u64) * 1000000000 + (ts.tv_nsec as u64)
        }

        #[derive(Copy)]
        pub struct SteadyTime {
            t: libc::timespec,
        }

        impl fmt::Debug for SteadyTime {
            fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
                write!(fmt, "SteadyTime {{ tv_sec: {:?}, tv_nsec: {:?} }}",
                       self.t.tv_sec, self.t.tv_nsec)
            }
        }

        impl Clone for SteadyTime {
            fn clone(&self) -> SteadyTime {
                SteadyTime { t: self.t }
            }
        }

        impl SteadyTime {
            pub fn now() -> SteadyTime {
                let mut t = SteadyTime {
                    t: libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }
                };
                unsafe {
                    assert_eq!(0, libc::clock_gettime(libc::CLOCK_MONOTONIC,
                                                      &mut t.t));
                }
                t
            }
        }

        impl Sub for SteadyTime {
            type Output = Duration;
            fn sub(self, other: SteadyTime) -> Duration {
                if self.t.tv_nsec >= other.t.tv_nsec {
                    Duration::seconds(self.t.tv_sec as i64 - other.t.tv_sec as i64) +
                        Duration::nanoseconds(self.t.tv_nsec as i64 - other.t.tv_nsec as i64)
                } else {
                    Duration::seconds(self.t.tv_sec as i64 - 1 - other.t.tv_sec as i64) +
                        Duration::nanoseconds(self.t.tv_nsec as i64 + ::NSEC_PER_SEC as i64 -
                                              other.t.tv_nsec as i64)
                }
            }
        }

        impl Sub<Duration> for SteadyTime {
            type Output = SteadyTime;
            fn sub(self, other: Duration) -> SteadyTime {
                self + -other
            }
        }

        impl Add<Duration> for SteadyTime {
            type Output = SteadyTime;
            fn add(mut self, other: Duration) -> SteadyTime {
                let seconds = other.num_seconds();
                let nanoseconds = other - Duration::seconds(seconds);
                let nanoseconds = nanoseconds.num_nanoseconds().unwrap();

                #[cfg(all(target_arch = "x86_64", target_pointer_width = "32"))]
                type nsec = i64;
                #[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
                type nsec = libc::c_long;

                self.t.tv_sec += seconds as libc::time_t;
                self.t.tv_nsec += nanoseconds as nsec;
                if self.t.tv_nsec >= ::NSEC_PER_SEC as nsec {
                    self.t.tv_nsec -= ::NSEC_PER_SEC as nsec;
                    self.t.tv_sec += 1;
                } else if self.t.tv_nsec < 0 {
                    self.t.tv_sec -= 1;
                    self.t.tv_nsec += ::NSEC_PER_SEC as nsec;
                }
                self
            }
        }

        impl PartialOrd for SteadyTime {
            fn partial_cmp(&self, other: &SteadyTime) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for SteadyTime {
            fn cmp(&self, other: &SteadyTime) -> Ordering {
                match self.t.tv_sec.cmp(&other.t.tv_sec) {
                    Ordering::Equal => self.t.tv_nsec.cmp(&other.t.tv_nsec),
                    ord => ord
                }
            }
        }

        impl PartialEq for SteadyTime {
            fn eq(&self, other: &SteadyTime) -> bool {
                self.t.tv_sec == other.t.tv_sec &&
                    self.t.tv_nsec == other.t.tv_nsec
            }
        }

        impl Eq for SteadyTime {}

    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
mod inner {
    use std::io;
    use std::mem;
    #[allow(deprecated)]
    use std::sync::{Once, ONCE_INIT};
    use std::ops::{Add, Sub};
    use {Tm, Duration};

    use winapi::um::winnt::*;
    use winapi::shared::minwindef::*;
    use winapi::um::minwinbase::SYSTEMTIME;
    use winapi::um::profileapi::*;
    use winapi::um::timezoneapi::*;
    use winapi::um::sysinfoapi::GetSystemTimeAsFileTime;

    fn frequency() -> i64 {
        static mut FREQUENCY: i64 = 0;
        #[allow(deprecated)]
        static ONCE: Once = ONCE_INIT;

        unsafe {
            ONCE.call_once(|| {
                let mut l = i64_to_large_integer(0);
                QueryPerformanceFrequency(&mut l);
                FREQUENCY = large_integer_to_i64(l);
            });
            FREQUENCY
        }
    }

    fn i64_to_large_integer(i: i64) -> LARGE_INTEGER {
        unsafe {
            let mut large_integer: LARGE_INTEGER = mem::zeroed();
            *large_integer.QuadPart_mut() = i;
            large_integer
        }
    }

    fn large_integer_to_i64(l: LARGE_INTEGER) -> i64 {
        unsafe {
            *l.QuadPart()
        }
    }

    const HECTONANOSECS_IN_SEC: i64 = 10_000_000;
    const HECTONANOSEC_TO_UNIX_EPOCH: i64 = 11_644_473_600 * HECTONANOSECS_IN_SEC;

    fn time_to_file_time(sec: i64) -> FILETIME {
        let t = (((sec * HECTONANOSECS_IN_SEC) + HECTONANOSEC_TO_UNIX_EPOCH)) as u64;
        FILETIME {
            dwLowDateTime: t as DWORD,
            dwHighDateTime: (t >> 32) as DWORD
        }
    }

    fn file_time_as_u64(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    fn file_time_to_nsec(ft: &FILETIME) -> i32 {
        let t = file_time_as_u64(ft) as i64;
        ((t % HECTONANOSECS_IN_SEC) * 100) as i32
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
                if year % 4 == 0 { 1 } else { 2 }
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

    pub fn time_to_utc_tm(sec: i64, tm: &mut Tm) {
        let mut out = unsafe { mem::zeroed() };
        let ft = time_to_file_time(sec);
        unsafe {
            call!(FileTimeToSystemTime(&ft, &mut out));
        }
        system_time_to_tm(&out, tm);
        tm.tm_utcoff = 0;
    }

    pub fn time_to_local_tm(sec: i64, tm: &mut Tm) {
        let ft = time_to_file_time(sec);
        unsafe {
            let mut utc = mem::zeroed();
            let mut local = mem::zeroed();
            call!(FileTimeToSystemTime(&ft, &mut utc));
            call!(SystemTimeToTzSpecificLocalTime(0 as *const _,
                                                  &mut utc, &mut local));
            system_time_to_tm(&local, tm);

            let local = system_time_to_file_time(&local);
            let local_sec = file_time_to_unix_seconds(&local);

            let mut tz = mem::zeroed();
            GetTimeZoneInformation(&mut tz);

            // SystemTimeToTzSpecificLocalTime already applied the biases so
            // check if it non standard
            tm.tm_utcoff = (local_sec - sec) as i32;
            tm.tm_isdst = if tm.tm_utcoff == -60 * (tz.Bias + tz.StandardBias) {
                0
            } else {
                1
            };
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
            call!(TzSpecificLocalTimeToSystemTime(0 as *mut _,
                                                  &mut sys_time, &mut utc));
            call!(SystemTimeToFileTime(&utc, &mut ft));
            file_time_to_unix_seconds(&ft)
        }
    }

    pub fn get_time() -> (i64, i32) {
        unsafe {
            let mut ft = mem::zeroed();
            GetSystemTimeAsFileTime(&mut ft);
            (file_time_to_unix_seconds(&ft), file_time_to_nsec(&ft))
        }
    }

    pub fn get_precise_ns() -> u64 {
        let mut ticks = i64_to_large_integer(0);
        unsafe {
            assert!(QueryPerformanceCounter(&mut ticks) == 1);
        }
        mul_div_i64(large_integer_to_i64(ticks), 1000000000, frequency()) as u64

    }

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub struct SteadyTime {
        t: i64,
    }

    impl SteadyTime {
        pub fn now() -> SteadyTime {
            let mut l = i64_to_large_integer(0);
            unsafe { QueryPerformanceCounter(&mut l); }
            SteadyTime { t : large_integer_to_i64(l) }
        }
    }

    impl Sub for SteadyTime {
        type Output = Duration;
        fn sub(self, other: SteadyTime) -> Duration {
            let diff = self.t as i64 - other.t as i64;
            Duration::nanoseconds(mul_div_i64(diff, 1000000000,
                                              frequency()))
        }
    }

    impl Sub<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn sub(self, other: Duration) -> SteadyTime {
            self + -other
        }
    }

    impl Add<Duration> for SteadyTime {
        type Output = SteadyTime;
        fn add(mut self, other: Duration) -> SteadyTime {
            self.t += (other.num_microseconds().unwrap() * frequency() /
                       1_000_000) as i64;
            self
        }
    }

    #[cfg(test)]
    pub struct TzReset {
        old: TIME_ZONE_INFORMATION,
    }

    #[cfg(test)]
    impl Drop for TzReset {
        fn drop(&mut self) {
            unsafe {
                call!(SetTimeZoneInformation(&self.old));
            }
        }
    }

    #[cfg(test)]
    pub fn set_los_angeles_time_zone() -> TzReset {
        acquire_privileges();

        unsafe {
            let mut tz = mem::zeroed::<TIME_ZONE_INFORMATION>();
            GetTimeZoneInformation(&mut tz);
            let ret = TzReset { old: tz };
            tz.Bias = 60 * 8;
            call!(SetTimeZoneInformation(&tz));
            return ret
        }
    }

    #[cfg(test)]
    pub fn set_london_with_dst_time_zone() -> TzReset {
        acquire_privileges();

        unsafe {
            let mut tz = mem::zeroed::<TIME_ZONE_INFORMATION>();
            GetTimeZoneInformation(&mut tz);
            let ret = TzReset { old: tz };
            // Since date set precisely this is 2015's dates
            tz.Bias = 0;
            tz.DaylightBias = -60;
            tz.DaylightDate.wYear = 0;
            tz.DaylightDate.wMonth = 3;
            tz.DaylightDate.wDayOfWeek = 0;
            tz.DaylightDate.wDay = 5;
            tz.DaylightDate.wHour = 2;
            tz.StandardBias = 0;
            tz.StandardDate.wYear = 0;
            tz.StandardDate.wMonth = 10;
            tz.StandardDate.wDayOfWeek = 0;
            tz.StandardDate.wDay = 5;
            tz.StandardDate.wHour = 2;
            call!(SetTimeZoneInformation(&tz));
            return ret
        }
    }

    // Ensures that this process has the necessary privileges to set a new time
    // zone, and this is all transcribed from:
    // https://msdn.microsoft.com/en-us/library/windows/desktop/ms724944%28v=vs.85%29.aspx
    #[cfg(test)]
    fn acquire_privileges() {
        use winapi::um::processthreadsapi::*;
        use winapi::um::winbase::LookupPrivilegeValueA;
        const SE_PRIVILEGE_ENABLED: DWORD = 2;
        #[allow(deprecated)]
        static INIT: Once = ONCE_INIT;

        // TODO: FIXME
        extern "system" {
            fn AdjustTokenPrivileges(
                TokenHandle: HANDLE, DisableAllPrivileges: BOOL, NewState: PTOKEN_PRIVILEGES,
                BufferLength: DWORD, PreviousState: PTOKEN_PRIVILEGES, ReturnLength: PDWORD,
            ) -> BOOL;
        }

        INIT.call_once(|| unsafe {
            let mut hToken = 0 as *mut _;
            call!(OpenProcessToken(GetCurrentProcess(),
                                   TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                                   &mut hToken));

            let mut tkp = mem::zeroed::<TOKEN_PRIVILEGES>();
            assert_eq!(tkp.Privileges.len(), 1);
            let c = ::std::ffi::CString::new("SeTimeZonePrivilege").unwrap();
            call!(LookupPrivilegeValueA(0 as *const _, c.as_ptr(),
                                        &mut tkp.Privileges[0].Luid));
            tkp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            tkp.PrivilegeCount = 1;
            call!(AdjustTokenPrivileges(hToken, FALSE, &mut tkp, 0,
                                        0 as *mut _, 0 as *mut _));
        });
    }



    // Computes (value*numer)/denom without overflow, as long as both
    // (numer*denom) and the overall result fit into i64 (which is the case
    // for our time conversions).
    fn mul_div_i64(value: i64, numer: i64, denom: i64) -> i64 {
        let q = value / denom;
        let r = value % denom;
        // Decompose value as (value/denom*denom + value%denom),
        // substitute into (value*numer)/denom and simplify.
        // r < denom, so (denom*numer) is the upper bound of (r*numer)
        q * numer + r * numer / denom
    }

    #[test]
    fn test_muldiv() {
        assert_eq!(mul_div_i64( 1_000_000_000_001, 1_000_000_000, 1_000_000),
                   1_000_000_000_001_000);
        assert_eq!(mul_div_i64(-1_000_000_000_001, 1_000_000_000, 1_000_000),
                   -1_000_000_000_001_000);
        assert_eq!(mul_div_i64(-1_000_000_000_001,-1_000_000_000, 1_000_000),
                   1_000_000_000_001_000);
        assert_eq!(mul_div_i64( 1_000_000_000_001, 1_000_000_000,-1_000_000),
                   -1_000_000_000_001_000);
        assert_eq!(mul_div_i64( 1_000_000_000_001,-1_000_000_000,-1_000_000),
                   1_000_000_000_001_000);
    }
}
