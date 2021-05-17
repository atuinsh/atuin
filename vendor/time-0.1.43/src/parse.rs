use super::{Timespec, Tm, at_utc, ParseError, NSEC_PER_SEC};

/// Parses the time from the string according to the format string.
pub fn strptime(mut s: &str, format: &str) -> Result<Tm, ParseError> {
    let mut tm = Tm {
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
    };
    let mut chars = format.chars();

    while let Some(ch) = chars.next() {
        if ch == '%' {
            if let Some(ch) = chars.next() {
                parse_type(&mut s, ch, &mut tm)?;
            }
        } else {
            parse_char(&mut s, ch)?;
        }
    }

    Ok(tm)
}

fn parse_type(s: &mut &str, ch: char, tm: &mut Tm) -> Result<(), ParseError> {
    match ch {
        'A' => match match_strs(s, &[("Sunday", 0),
                                     ("Monday", 1),
                                     ("Tuesday", 2),
                                     ("Wednesday", 3),
                                     ("Thursday", 4),
                                     ("Friday", 5),
                                     ("Saturday", 6)]) {
            Some(v) => { tm.tm_wday = v; Ok(()) }
            None => Err(ParseError::InvalidDay)
        },
        'a' => match match_strs(s, &[("Sun", 0),
                                     ("Mon", 1),
                                     ("Tue", 2),
                                     ("Wed", 3),
                                     ("Thu", 4),
                                     ("Fri", 5),
                                     ("Sat", 6)]) {
            Some(v) => { tm.tm_wday = v; Ok(()) }
            None => Err(ParseError::InvalidDay)
        },
        'B' => match match_strs(s, &[("January", 0),
                                     ("February", 1),
                                     ("March", 2),
                                     ("April", 3),
                                     ("May", 4),
                                     ("June", 5),
                                     ("July", 6),
                                     ("August", 7),
                                     ("September", 8),
                                     ("October", 9),
                                     ("November", 10),
                                     ("December", 11)]) {
            Some(v) => { tm.tm_mon = v; Ok(()) }
            None => Err(ParseError::InvalidMonth)
        },
        'b' | 'h' => match match_strs(s, &[("Jan", 0),
                                           ("Feb", 1),
                                           ("Mar", 2),
                                           ("Apr", 3),
                                           ("May", 4),
                                           ("Jun", 5),
                                           ("Jul", 6),
                                           ("Aug", 7),
                                           ("Sep", 8),
                                           ("Oct", 9),
                                           ("Nov", 10),
                                           ("Dec", 11)]) {
            Some(v) => { tm.tm_mon = v; Ok(()) }
            None => Err(ParseError::InvalidMonth)
        },
        'C' => match match_digits_in_range(s, 1, 2, false, 0, 99) {
            Some(v) => { tm.tm_year += (v * 100) - 1900; Ok(()) }
            None => Err(ParseError::InvalidYear)
        },
        'c' => {
            parse_type(s, 'a', tm)
                .and_then(|()| parse_char(s, ' '))
                .and_then(|()| parse_type(s, 'b', tm))
                .and_then(|()| parse_char(s, ' '))
                .and_then(|()| parse_type(s, 'e', tm))
                .and_then(|()| parse_char(s, ' '))
                .and_then(|()| parse_type(s, 'T', tm))
                .and_then(|()| parse_char(s, ' '))
                .and_then(|()| parse_type(s, 'Y', tm))
        }
        'D' | 'x' => {
            parse_type(s, 'm', tm)
                .and_then(|()| parse_char(s, '/'))
                .and_then(|()| parse_type(s, 'd', tm))
                .and_then(|()| parse_char(s, '/'))
                .and_then(|()| parse_type(s, 'y', tm))
        }
        'd' => match match_digits_in_range(s, 1, 2, false, 1, 31) {
            Some(v) => { tm.tm_mday = v; Ok(()) }
            None => Err(ParseError::InvalidDayOfMonth)
        },
        'e' => match match_digits_in_range(s, 1, 2, true, 1, 31) {
            Some(v) => { tm.tm_mday = v; Ok(()) }
            None => Err(ParseError::InvalidDayOfMonth)
        },
        'f' => {
            tm.tm_nsec = match_fractional_seconds(s);
            Ok(())
        }
        'F' => {
            parse_type(s, 'Y', tm)
                .and_then(|()| parse_char(s, '-'))
                .and_then(|()| parse_type(s, 'm', tm))
                .and_then(|()| parse_char(s, '-'))
                .and_then(|()| parse_type(s, 'd', tm))
        }
        'H' => {
            match match_digits_in_range(s, 1, 2, false, 0, 23) {
                Some(v) => { tm.tm_hour = v; Ok(()) }
                None => Err(ParseError::InvalidHour)
            }
        }
        'I' => {
            match match_digits_in_range(s, 1, 2, false, 1, 12) {
                Some(v) => { tm.tm_hour = if v == 12 { 0 } else { v }; Ok(()) }
                None => Err(ParseError::InvalidHour)
            }
        }
        'j' => {
            match match_digits_in_range(s, 1, 3, false, 1, 366) {
                Some(v) => { tm.tm_yday = v - 1; Ok(()) }
                None => Err(ParseError::InvalidDayOfYear)
            }
        }
        'k' => {
            match match_digits_in_range(s, 1, 2, true, 0, 23) {
                Some(v) => { tm.tm_hour = v; Ok(()) }
                None => Err(ParseError::InvalidHour)
            }
        }
        'l' => {
            match match_digits_in_range(s, 1, 2, true, 1, 12) {
                Some(v) => { tm.tm_hour = if v == 12 { 0 } else { v }; Ok(()) }
                None => Err(ParseError::InvalidHour)
            }
        }
        'M' => {
            match match_digits_in_range(s, 1, 2, false, 0, 59) {
                Some(v) => { tm.tm_min = v; Ok(()) }
                None => Err(ParseError::InvalidMinute)
            }
        }
        'm' => {
            match match_digits_in_range(s, 1, 2, false, 1, 12) {
                Some(v) => { tm.tm_mon = v - 1; Ok(()) }
                None => Err(ParseError::InvalidMonth)
            }
        }
        'n' => parse_char(s, '\n'),
        'P' => match match_strs(s, &[("am", 0), ("pm", 12)]) {
            Some(v) => { tm.tm_hour += v; Ok(()) }
            None => Err(ParseError::InvalidHour)
        },
        'p' => match match_strs(s, &[("AM", 0), ("PM", 12)]) {
            Some(v) => { tm.tm_hour += v; Ok(()) }
            None => Err(ParseError::InvalidHour)
        },
        'R' => {
            parse_type(s, 'H', tm)
                .and_then(|()| parse_char(s, ':'))
                .and_then(|()| parse_type(s, 'M', tm))
        }
        'r' => {
            parse_type(s, 'I', tm)
                .and_then(|()| parse_char(s, ':'))
                .and_then(|()| parse_type(s, 'M', tm))
                .and_then(|()| parse_char(s, ':'))
                .and_then(|()| parse_type(s, 'S', tm))
                .and_then(|()| parse_char(s, ' '))
                .and_then(|()| parse_type(s, 'p', tm))
        }
        's' => {
            match match_digits_i64(s, 1, 18, false) {
                Some(v) => {
                    *tm = at_utc(Timespec::new(v, 0));
                    Ok(())
                },
                None => Err(ParseError::InvalidSecondsSinceEpoch)
            }
        }
        'S' => {
            match match_digits_in_range(s, 1, 2, false, 0, 60) {
                Some(v) => { tm.tm_sec = v; Ok(()) }
                None => Err(ParseError::InvalidSecond)
            }
        }
        //'s' {}
        'T' | 'X' => {
            parse_type(s, 'H', tm)
                .and_then(|()| parse_char(s, ':'))
                .and_then(|()| parse_type(s, 'M', tm))
                .and_then(|()| parse_char(s, ':'))
                .and_then(|()| parse_type(s, 'S', tm))
        }
        't' => parse_char(s, '\t'),
        'u' => {
            match match_digits_in_range(s, 1, 1, false, 1, 7) {
                Some(v) => { tm.tm_wday = if v == 7 { 0 } else { v }; Ok(()) }
                None => Err(ParseError::InvalidDayOfWeek)
            }
        }
        'v' => {
            parse_type(s, 'e', tm)
                .and_then(|()| parse_char(s, '-'))
                .and_then(|()| parse_type(s, 'b', tm))
                .and_then(|()| parse_char(s, '-'))
                .and_then(|()| parse_type(s, 'Y', tm))
        }
        //'W' {}
        'w' => {
            match match_digits_in_range(s, 1, 1, false, 0, 6) {
                Some(v) => { tm.tm_wday = v; Ok(()) }
                None => Err(ParseError::InvalidDayOfWeek)
            }
        }
        'Y' => {
            match match_digits(s, 4, 4, false) {
                Some(v) => { tm.tm_year = v - 1900; Ok(()) }
                None => Err(ParseError::InvalidYear)
            }
        }
        'y' => {
            match match_digits_in_range(s, 1, 2, false, 0, 99) {
                Some(v) => { tm.tm_year = v; Ok(()) }
                None => Err(ParseError::InvalidYear)
            }
        }
        'Z' => {
            if match_str(s, "UTC") || match_str(s, "GMT") {
                tm.tm_utcoff = 0;
                Ok(())
            } else {
                // It's odd, but to maintain compatibility with c's
                // strptime we ignore the timezone.
                for (i, ch) in s.char_indices() {
                    if ch == ' ' {
                        *s = &s[i..];
                        return Ok(())
                    }
                }
                *s = "";
                Ok(())
            }
        }
        'z' => {
            if parse_char(s, 'Z').is_ok() {
                tm.tm_utcoff = 0;
                Ok(())
            } else {
                let sign = if parse_char(s, '+').is_ok() {1}
                           else if parse_char(s, '-').is_ok() {-1}
                           else { return Err(ParseError::InvalidZoneOffset) };

                let hours;
                let minutes;

                match match_digits(s, 2, 2, false) {
                    Some(h) => hours = h,
                    None => return Err(ParseError::InvalidZoneOffset)
                }

                // consume the colon if its present,
                // just ignore it otherwise
                let _ = parse_char(s, ':');

                match match_digits(s, 2, 2, false) {
                    Some(m) => minutes = m,
                    None => return Err(ParseError::InvalidZoneOffset)
                }

                tm.tm_utcoff = sign * (hours * 60 * 60 + minutes * 60);
                Ok(())
            }
        }
        '%' => parse_char(s, '%'),
        ch => Err(ParseError::InvalidFormatSpecifier(ch))
    }
}


fn match_str(s: &mut &str, needle: &str) -> bool {
    if s.starts_with(needle) {
        *s = &s[needle.len()..];
        true
    } else {
        false
    }
}

fn match_strs(ss: &mut &str, strs: &[(&str, i32)]) -> Option<i32> {
    for &(needle, value) in strs.iter() {
        if match_str(ss, needle) {
            return Some(value)
        }
    }
    None
}

fn match_digits(ss: &mut &str, min_digits : usize, max_digits: usize, ws: bool) -> Option<i32> {
    match match_digits_i64(ss, min_digits, max_digits, ws) {
        Some(v) => Some(v as i32),
        None => None
    }
}

fn match_digits_i64(ss: &mut &str, min_digits : usize, max_digits: usize, ws: bool) -> Option<i64> {
    let mut value : i64 = 0;
    let mut n = 0;
    if ws {
        #[allow(deprecated)] // use `trim_start_matches` starting in 1.30
        let s2 = ss.trim_left_matches(" ");
        n = ss.len() - s2.len();
        if n > max_digits { return None }
    }
    let chars = ss[n..].char_indices();
    for (_, ch) in chars.take(max_digits - n) {
        match ch {
            '0' ... '9' => value = value * 10 + (ch as i64 - '0' as i64),
            _ => break,
        }
        n += 1;
    }

    if n >= min_digits && n <= max_digits {
        *ss = &ss[n..];
        Some(value)
    } else {
        None
    }
}

fn match_fractional_seconds(ss: &mut &str) -> i32 {
    let mut value = 0;
    let mut multiplier = NSEC_PER_SEC / 10;

    let mut chars = ss.char_indices();
    let orig = *ss;
    for (i, ch) in &mut chars {
        *ss = &orig[i..];
        match ch {
            '0' ... '9' => {
                // This will drop digits after the nanoseconds place
                let digit = ch as i32 - '0' as i32;
                value += digit * multiplier;
                multiplier /= 10;
            }
            _ => break
        }
    }

    value
}

fn match_digits_in_range(ss: &mut &str,
                         min_digits : usize, max_digits : usize,
                         ws: bool, min: i32, max: i32) -> Option<i32> {
    let before = *ss;
    match match_digits(ss, min_digits, max_digits, ws) {
        Some(val) if val >= min && val <= max => Some(val),
        _ => { *ss = before; None }
    }
}

fn parse_char(s: &mut &str, c: char) -> Result<(), ParseError> {
    match s.char_indices().next() {
        Some((i, c2)) => {
            if c == c2 {
                *s = &s[i + c2.len_utf8()..];
                Ok(())
            } else {
                Err(ParseError::UnexpectedCharacter(c, c2))
            }
        }
        None => Err(ParseError::InvalidTime),
    }
}
