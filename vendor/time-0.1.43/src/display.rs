use std::fmt::{self, Write};

use super::{TmFmt, Tm, Fmt};

impl<'a> fmt::Display for TmFmt<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self.format {
            Fmt::Str(ref s) => {
                let mut chars = s.chars();
                while let Some(ch) = chars.next() {
                    if ch == '%' {
                        // we've already validated that % always precedes
                        // another char
                        parse_type(fmt, chars.next().unwrap(), self.tm)?;
                    } else {
                        fmt.write_char(ch)?;
                    }
                }

                Ok(())
            }
            Fmt::Ctime => self.tm.to_local().asctime().fmt(fmt),
            Fmt::Rfc3339 => {
                if self.tm.tm_utcoff == 0 {
                    TmFmt {
                        tm: self.tm,
                        format: Fmt::Str("%Y-%m-%dT%H:%M:%SZ"),
                    }.fmt(fmt)
                } else {
                    let s = TmFmt {
                        tm: self.tm,
                        format: Fmt::Str("%Y-%m-%dT%H:%M:%S"),
                    };
                    let sign = if self.tm.tm_utcoff > 0 { '+' } else { '-' };
                    let mut m = abs(self.tm.tm_utcoff) / 60;
                    let h = m / 60;
                    m -= h * 60;
                    write!(fmt, "{}{}{:02}:{:02}", s, sign, h, m)
                }
            }
        }
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0) && ((year % 100 != 0) || (year % 400 == 0))
}

fn days_in_year(year: i32) -> i32 {
    if is_leap_year(year) { 366 }
    else                  { 365 }
}

fn iso_week_days(yday: i32, wday: i32) -> i32 {
    /* The number of days from the first day of the first ISO week of this
    * year to the year day YDAY with week day WDAY.
    * ISO weeks start on Monday. The first ISO week has the year's first
    * Thursday.
    * YDAY may be as small as yday_minimum.
    */
    let iso_week_start_wday: i32 = 1;                     /* Monday */
    let iso_week1_wday: i32 = 4;                          /* Thursday */
    let yday_minimum: i32 = 366;
    /* Add enough to the first operand of % to make it nonnegative. */
    let big_enough_multiple_of_7: i32 = (yday_minimum / 7 + 2) * 7;

    yday - (yday - wday + iso_week1_wday + big_enough_multiple_of_7) % 7
        + iso_week1_wday - iso_week_start_wday
}

fn iso_week(fmt: &mut fmt::Formatter, ch:char, tm: &Tm) -> fmt::Result {
    let mut year = tm.tm_year + 1900;
    let mut days = iso_week_days(tm.tm_yday, tm.tm_wday);

    if days < 0 {
        /* This ISO week belongs to the previous year. */
        year -= 1;
        days = iso_week_days(tm.tm_yday + (days_in_year(year)), tm.tm_wday);
    } else {
        let d = iso_week_days(tm.tm_yday - (days_in_year(year)),
                              tm.tm_wday);
        if 0 <= d {
            /* This ISO week belongs to the next year. */
            year += 1;
            days = d;
        }
    }

    match ch {
        'G' => write!(fmt, "{}", year),
        'g' => write!(fmt, "{:02}", (year % 100 + 100) % 100),
        'V' => write!(fmt, "{:02}", days / 7 + 1),
        _ => Ok(())
    }
}

fn parse_type(fmt: &mut fmt::Formatter, ch: char, tm: &Tm) -> fmt::Result {
    match ch {
        'A' => fmt.write_str(match tm.tm_wday {
            0 => "Sunday",
            1 => "Monday",
            2 => "Tuesday",
            3 => "Wednesday",
            4 => "Thursday",
            5 => "Friday",
            6 => "Saturday",
            _ => unreachable!(),
        }),
        'a' => fmt.write_str(match tm.tm_wday {
            0 => "Sun",
            1 => "Mon",
            2 => "Tue",
            3 => "Wed",
            4 => "Thu",
            5 => "Fri",
            6 => "Sat",
            _ => unreachable!(),
        }),
        'B' => fmt.write_str(match tm.tm_mon {
            0 => "January",
            1 => "February",
            2 => "March",
            3 => "April",
            4 => "May",
            5 => "June",
            6 => "July",
            7 => "August",
            8 => "September",
            9 => "October",
            10 => "November",
            11 => "December",
            _ => unreachable!(),
        }),
        'b' | 'h' => fmt.write_str(match tm.tm_mon {
            0 => "Jan",
            1 => "Feb",
            2 => "Mar",
            3 => "Apr",
            4 => "May",
            5 => "Jun",
            6 => "Jul",
            7 => "Aug",
            8 => "Sep",
            9 => "Oct",
            10 => "Nov",
            11 => "Dec",
            _  => unreachable!(),
        }),
        'C' => write!(fmt, "{:02}", (tm.tm_year + 1900) / 100),
        'c' => {
            parse_type(fmt, 'a', tm)?;
            fmt.write_str(" ")?;
            parse_type(fmt, 'b', tm)?;
            fmt.write_str(" ")?;
            parse_type(fmt, 'e', tm)?;
            fmt.write_str(" ")?;
            parse_type(fmt, 'T', tm)?;
            fmt.write_str(" ")?;
            parse_type(fmt, 'Y', tm)
        }
        'D' | 'x' => {
            parse_type(fmt, 'm', tm)?;
            fmt.write_str("/")?;
            parse_type(fmt, 'd', tm)?;
            fmt.write_str("/")?;
            parse_type(fmt, 'y', tm)
        }
        'd' => write!(fmt, "{:02}", tm.tm_mday),
        'e' => write!(fmt, "{:2}", tm.tm_mday),
        'f' => write!(fmt, "{:09}", tm.tm_nsec),
        'F' => {
            parse_type(fmt, 'Y', tm)?;
            fmt.write_str("-")?;
            parse_type(fmt, 'm', tm)?;
            fmt.write_str("-")?;
            parse_type(fmt, 'd', tm)
        }
        'G' => iso_week(fmt, 'G', tm),
        'g' => iso_week(fmt, 'g', tm),
        'H' => write!(fmt, "{:02}", tm.tm_hour),
        'I' => {
            let mut h = tm.tm_hour;
            if h == 0 { h = 12 }
            if h > 12 { h -= 12 }
            write!(fmt, "{:02}", h)
        }
        'j' => write!(fmt, "{:03}", tm.tm_yday + 1),
        'k' => write!(fmt, "{:2}", tm.tm_hour),
        'l' => {
            let mut h = tm.tm_hour;
            if h == 0 { h = 12 }
            if h > 12 { h -= 12 }
            write!(fmt, "{:2}", h)
        }
        'M' => write!(fmt, "{:02}", tm.tm_min),
        'm' => write!(fmt, "{:02}", tm.tm_mon + 1),
        'n' => fmt.write_str("\n"),
        'P' => fmt.write_str(if tm.tm_hour < 12 { "am" } else { "pm" }),
        'p' => fmt.write_str(if (tm.tm_hour) < 12 { "AM" } else { "PM" }),
        'R' => {
            parse_type(fmt, 'H', tm)?;
            fmt.write_str(":")?;
            parse_type(fmt, 'M', tm)
        }
        'r' => {
            parse_type(fmt, 'I', tm)?;
            fmt.write_str(":")?;
            parse_type(fmt, 'M', tm)?;
            fmt.write_str(":")?;
            parse_type(fmt, 'S', tm)?;
            fmt.write_str(" ")?;
            parse_type(fmt, 'p', tm)
        }
        'S' => write!(fmt, "{:02}", tm.tm_sec),
        's' => write!(fmt, "{}", tm.to_timespec().sec),
        'T' | 'X' => {
            parse_type(fmt, 'H', tm)?;
            fmt.write_str(":")?;
            parse_type(fmt, 'M', tm)?;
            fmt.write_str(":")?;
            parse_type(fmt, 'S', tm)
        }
        't' => fmt.write_str("\t"),
        'U' => write!(fmt, "{:02}", (tm.tm_yday - tm.tm_wday + 7) / 7),
        'u' => {
            let i = tm.tm_wday;
            write!(fmt, "{}", (if i == 0 { 7 } else { i }))
        }
        'V' => iso_week(fmt, 'V', tm),
        'v' => {
            parse_type(fmt, 'e', tm)?;
            fmt.write_str("-")?;
            parse_type(fmt, 'b', tm)?;
            fmt.write_str("-")?;
            parse_type(fmt, 'Y', tm)
        }
        'W' => {
            write!(fmt, "{:02}", (tm.tm_yday - (tm.tm_wday - 1 + 7) % 7 + 7) / 7)
        }
        'w' => write!(fmt, "{}", tm.tm_wday),
        'Y' => write!(fmt, "{}", tm.tm_year + 1900),
        'y' => write!(fmt, "{:02}", (tm.tm_year + 1900) % 100),
        // FIXME (#2350): support locale
        'Z' => fmt.write_str(if tm.tm_utcoff == 0 { "UTC"} else { "" }),
        'z' => {
            let sign = if tm.tm_utcoff > 0 { '+' } else { '-' };
            let mut m = abs(tm.tm_utcoff) / 60;
            let h = m / 60;
            m -= h * 60;
            write!(fmt, "{}{:02}{:02}", sign, h, m)
        }
        '+' => write!(fmt, "{}", tm.rfc3339()),
        '%' => fmt.write_str("%"),
        _   => unreachable!(),
    }
}

fn abs(i: i32) -> i32 {
    if i < 0 {-i} else {i}
}
