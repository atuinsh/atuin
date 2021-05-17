use scanlex::{Scanner,Token};
use errors::*;
use types::*;

// when we parse dates, there's often a bit of time parsed..
#[derive(Clone,Copy,Debug)]
enum TimeKind {
    Formal,
    Informal,
    AmPm(bool),
    Unknown,
}

pub struct DateParser<'a> {
    s: Scanner<'a>,
    direct: Direction,
    maybe_time: Option<(u32,TimeKind)>,
    pub american: bool, // 9/11, not 20/03
}

impl <'a> DateParser<'a> {

    pub fn new(text: &'a str) -> DateParser<'a> {
        DateParser{
            s: Scanner::new(text).no_float(),
            direct: Direction::Here,
            maybe_time: None,
            american: false
        }
    }

    pub fn american_date(mut self) -> DateParser<'a> {
        self.american = true;
        self
    }

    fn iso_date(&mut self, y: u32) -> DateResult<DateSpec> {
        let month = self.s.get_int::<u32>()?;
        self.s.get_ch_matching(&['-'])?;
        let day = self.s.get_int::<u32>()?;
        Ok(DateSpec::absolute(y,month,day))
    }

    fn informal_date(&mut self, day_or_month: u32) -> DateResult<DateSpec> {
        let month_or_day = self.s.get_int::<u32>()?;
        let (d,m) = if self.american {
            (month_or_day, day_or_month)
        } else {
            (day_or_month, month_or_day)
        };
        Ok(if self.s.peek() == '/' {
            self.s.get();
            let y = self.s.get_int::<u32>()?;
            let y = if y < 100 { // pivot (1940, 2040)
                if y > 40 {
                    1900 + y
                } else {
                    2000 + y
                }
            } else {
                y
            };
            DateSpec::absolute(y,m,d)
        } else {
            DateSpec::FromName(ByName::from_day_month(d,m,self.direct))
        })
    }

    fn parse_date(&mut self) -> DateResult<Option<DateSpec>> {
        let mut t = self.s.next().or_err("empty date string")?;

        let sign = if t.is_char() && t.as_char().unwrap() == '-' {
            true
        } else {
            false
        };
        if sign {
            t = self.s.next().or_err("nothing after '-'")?;
        }
        if let Some(name) = t.as_iden() {
            let shortcut = match name {
                "now" => Some(0),
                "today" => Some(0),
                "yesterday" => Some(-1),
                "tomorrow" => Some(1),
                _ => None
            };
            if let Some(skip) = shortcut {
                return Ok(Some(
                    DateSpec::skip(time_unit("day").unwrap(), skip)
                ));
            } else // maybe next or last?
            if let Some(d) = Direction::from_name(&name) {
                self.direct = d;
            }
        }
        if self.direct != Direction::Here {
            t = self.s.next().or_err("nothing after last/next")?;
        }
        Ok(match t {
            Token::Iden(ref name) => {
                let name = name.to_lowercase();
                // maybe weekday or month name?
                if let Some(by_name) = ByName::from_name(&name,self.direct) {
                    // however, MONTH _might_ be followed by DAY, YEAR
                    if let Some(month) = by_name.as_month() {
                        let t = self.s.get();
                        if t.is_integer() {
                            let day = t.to_int_result::<u32>()?;
                            return Ok(Some(if self.s.peek() == ',' {
                                self.s.get_char()?; // eat ','
                                let year = self.s.get_int::<u32>()?;
                                DateSpec::absolute(year,month,day)
                            } else { // MONTH DAY is like DAY MONTH (tho no time!)
                                DateSpec::from_day_month(day, month, self.direct)
                            }));
                        }
                    }
                    Some(DateSpec::FromName(by_name))
                } else {
                    return date_result("expected week day or month name");
                }
            },
            Token::Int(_) => {
                let n = t.to_int_result::<u32>()?;
                let t = self.s.get();
                if t.finished() { // must be a year...
                    return Ok(Some(DateSpec::absolute(n,1,1)));
                }
                match t {
                    Token::Iden(ref name) => {
                        let day = n;
                        let name = name.to_lowercase();
                        if let Some(month) = month_name(&name) {
                            if let Ok(year) = self.s.get_int::<u32>() {
                                // 4 July 2017
                                Some(DateSpec::absolute(year,month,day))
                            } else {
                                // 4 July
                                Some(DateSpec::from_day_month(day, month, self.direct))
                            }
                        } else
                        if let Some(u) = time_unit(&name) { // '2 days'
                            let mut n = n as i32;
                            if sign {
                                n = -n;
                            } else {
                                let t = self.s.get();
                                let got_ago = if let Some(name) = t.as_iden() {
                                    if name == "ago" {
                                        n = -n;
                                        true
                                    } else {
                                        return date_result("only expected 'ago'");
                                    }
                                } else {
                                    false
                                };
                                if ! got_ago {
                                    if let Some(h) = t.to_integer() {
                                        self.maybe_time = Some((h as u32, TimeKind::Unknown));
                                    }
                                }
                            }
                            Some(DateSpec::skip(u, n))
                        } else
                        if name == "am" || name == "pm" {
                            self.maybe_time = Some((n, TimeKind::AmPm(name == "pm")));
                            None
                        } else {
                            return date_result("expected month or time unit");
                        }
                    },
                    Token::Char(ch) => {
                        match ch {
                            '-' => Some(self.iso_date(n)?),
                            '/' => Some(self.informal_date(n)?),
                            ':' | '.' => {
                                let kind = if ch == ':' {
                                    TimeKind::Formal
                                } else {
                                    TimeKind::Informal
                                };
                                self.maybe_time = Some((n,kind));
                                None
                            }
                            _ => return date_result(&format!("unexpected char {:?}",ch)),
                        }
                    },
                    _ => return date_result(&format!("unexpected token {:?}",t)),

                }
            },
            _ => return date_result(&format!("not expected token {:?}",t)),
        })

    }

    fn formal_time(&mut self, hour: u32) -> DateResult<TimeSpec> {
        let min = self.s.get_int::<u32>()?;
        // minute may be followed by [:secs][am|pm]
        let mut tnext = None;
        let sec = if let Some(t) = self.s.next() {
            if let Some(ch) = t.as_char() {
                if ch != ':' {
                    return date_result("expecting ':'");
                }
                self.s.get_int::<u32>()?
            } else {
                tnext = Some(t);
                0
            }
        } else {
            0
        };
        // we found seconds, look ahead
        if tnext.is_none() {
            tnext = self.s.next();
        }
        let micros = if let Some(Some('.')) = tnext.as_ref().map(|t| t.as_char()) {
            let frac = self.s.grab_while(char::is_numeric);
            if frac.is_empty() {
                return date_result("expected fractional second after '.'");
            }
            let frac = "0.".to_owned() + &frac;
            let micros_f = frac.parse::<f64>().unwrap() * 1.0e6;
            tnext = self.s.next();
            micros_f as u32
        } else {
            0
        };
        if tnext.is_none() {
            Ok(TimeSpec::new(hour, min, sec, micros ))
        } else {
            let tok = tnext.as_ref().unwrap();
            if let Some(ch) = tok.as_char() {
                let expecting_offset = match ch {
                    '+' | '-' => true,
                    _ => return date_result("expected +/- before timezone")
                };
                let offset = if expecting_offset {
                    let h = self.s.get_int::<u32>()?;
                    let (h, m) = if self.s.peek() == ':' { // 02:00
                        self.s.nextch();
                        (h, self.s.get_int::<u32>()?)
                    } else { // 0030 ....
                        let hh = h;
                        let h = hh / 100;
                        let m = hh % 100;
                        (h, m)
                    };
                    let res = 60 * (m + 60 * h);
                    (res as i64) * if ch == '-' { -1 } else { 1 }
                } else {
                    0
                };
                Ok(TimeSpec::new_with_offset(hour, min, sec, offset,micros))
            } else if let Some(id) = tok.as_iden() {
                if id == "Z" {
                    Ok(TimeSpec::new_with_offset(hour,min,sec,0,micros))
                } else { // am or pm
                    let hour = DateParser::am_pm(&id, hour)?;
                    Ok(TimeSpec::new(hour, min, sec, micros))
                }
            } else {
                Ok(TimeSpec::new(hour, min, sec, micros))
            }
        }
    }

    fn informal_time(&mut self, hour: u32) -> DateResult<TimeSpec> {
        let min = self.s.get_int::<u32>()?;
        let hour = if let Some(t) = self.s.next() {
            let name = t.to_iden_result()?;
            DateParser::am_pm(&name,hour)?
        } else {
            hour
        };
        Ok(TimeSpec::new(hour, min, 0, 0))
    }

    fn am_pm(name: &str, mut hour: u32) -> DateResult<u32> {
        if name == "pm" {
            hour += 12;
        } else
        if name != "am" {
            return date_result("expected am or pm");
        }
        Ok(hour)
    }

    fn hour_time(name: &str, hour: u32) -> DateResult<TimeSpec> {
        Ok(TimeSpec::new (DateParser::am_pm(name, hour)?, 0, 0, 0))
    }

    fn parse_time(&mut self) -> DateResult<Option<TimeSpec>> {
        // here the date parser looked ahead and saw an hour followed by some separator
        if let Some(hour_sep) = self.maybe_time { // didn't see a separator, so look...
            let (h,mut kind) = hour_sep;
            if let TimeKind::Unknown = kind {
                kind = match self.s.get_char()? {
                    ':' => TimeKind::Formal,
                    '.' => TimeKind::Informal,
                    ch => return date_result(&format!("expected : or ., not {}", ch)),
                };
            }
            Ok(Some(
                match kind {
                    TimeKind::Formal => self.formal_time(h)?,
                    TimeKind::Informal => self.informal_time(h)?,
                    TimeKind::AmPm(is_pm) =>
                        DateParser::hour_time(if is_pm {"pm"} else {"am"},h)?,
                    TimeKind::Unknown => unreachable!(),
                }
            ))
        } else { // no lookahead...
            if self.s.peek() == 'T' {
                self.s.nextch();
            }
            let t = self.s.get();
            if t.finished() {
                return Ok(None);
            }

            let hour = t.to_int_result::<u32>()?;
            Ok(Some(match self.s.get() {
                Token::Char(ch) => match ch {
                    ':' => self.formal_time(hour)?,
                    '.' => self.informal_time(hour)?,
                    ch => return date_result(&format!("unexpected char {:?}",ch))
                },
                Token::Iden(name) => {
                    DateParser::hour_time(&name,hour)?
                }
                t => return date_result(&format!("unexpected token {:?}",t))
            }))
        }
    }

    pub fn parse(&mut self) -> DateResult<DateTimeSpec> {
        let date = self.parse_date()?;
        let time = self.parse_time()?;
        Ok(DateTimeSpec{date: date, time: time})
    }

}
