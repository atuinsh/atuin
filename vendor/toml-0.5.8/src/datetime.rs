use std::error;
use std::fmt;
use std::str::{self, FromStr};

use serde::{de, ser};

/// A parsed TOML datetime value
///
/// This structure is intended to represent the datetime primitive type that can
/// be encoded into TOML documents. This type is a parsed version that contains
/// all metadata internally.
///
/// Currently this type is intentionally conservative and only supports
/// `to_string` as an accessor. Over time though it's intended that it'll grow
/// more support!
///
/// Note that if you're using `Deserialize` to deserialize a TOML document, you
/// can use this as a placeholder for where you're expecting a datetime to be
/// specified.
///
/// Also note though that while this type implements `Serialize` and
/// `Deserialize` it's only recommended to use this type with the TOML format,
/// otherwise encoded in other formats it may look a little odd.
#[derive(PartialEq, Clone)]
pub struct Datetime {
    date: Option<Date>,
    time: Option<Time>,
    offset: Option<Offset>,
}

/// Error returned from parsing a `Datetime` in the `FromStr` implementation.
#[derive(Debug, Clone)]
pub struct DatetimeParseError {
    _private: (),
}

// Currently serde itself doesn't have a datetime type, so we map our `Datetime`
// to a special valid in the serde data model. Namely one with thiese special
// fields/struct names.
//
// In general the TOML encoder/decoder will catch this and not literally emit
// these strings but rather emit datetimes as they're intended.
pub const FIELD: &str = "$__toml_private_datetime";
pub const NAME: &str = "$__toml_private_Datetime";

#[derive(PartialEq, Clone)]
struct Date {
    year: u16,
    month: u8,
    day: u8,
}

#[derive(PartialEq, Clone)]
struct Time {
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
}

#[derive(PartialEq, Clone)]
enum Offset {
    Z,
    Custom { hours: i8, minutes: u8 },
}

impl fmt::Debug for Datetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Datetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref date) = self.date {
            write!(f, "{}", date)?;
        }
        if let Some(ref time) = self.time {
            if self.date.is_some() {
                write!(f, "T")?;
            }
            write!(f, "{}", time)?;
        }
        if let Some(ref offset) = self.offset {
            write!(f, "{}", offset)?;
        }
        Ok(())
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.nanosecond != 0 {
            let s = format!("{:09}", self.nanosecond);
            write!(f, ".{}", s.trim_end_matches('0'))?;
        }
        Ok(())
    }
}

impl fmt::Display for Offset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Offset::Z => write!(f, "Z"),
            Offset::Custom { hours, minutes } => write!(f, "{:+03}:{:02}", hours, minutes),
        }
    }
}

impl FromStr for Datetime {
    type Err = DatetimeParseError;

    fn from_str(date: &str) -> Result<Datetime, DatetimeParseError> {
        // Accepted formats:
        //
        // 0000-00-00T00:00:00.00Z
        // 0000-00-00T00:00:00.00
        // 0000-00-00
        // 00:00:00.00
        if date.len() < 3 {
            return Err(DatetimeParseError { _private: () });
        }
        let mut offset_allowed = true;
        let mut chars = date.chars();

        // First up, parse the full date if we can
        let full_date = if chars.clone().nth(2) == Some(':') {
            offset_allowed = false;
            None
        } else {
            let y1 = u16::from(digit(&mut chars)?);
            let y2 = u16::from(digit(&mut chars)?);
            let y3 = u16::from(digit(&mut chars)?);
            let y4 = u16::from(digit(&mut chars)?);

            match chars.next() {
                Some('-') => {}
                _ => return Err(DatetimeParseError { _private: () }),
            }

            let m1 = digit(&mut chars)?;
            let m2 = digit(&mut chars)?;

            match chars.next() {
                Some('-') => {}
                _ => return Err(DatetimeParseError { _private: () }),
            }

            let d1 = digit(&mut chars)?;
            let d2 = digit(&mut chars)?;

            let date = Date {
                year: y1 * 1000 + y2 * 100 + y3 * 10 + y4,
                month: m1 * 10 + m2,
                day: d1 * 10 + d2,
            };

            if date.month < 1 || date.month > 12 {
                return Err(DatetimeParseError { _private: () });
            }
            if date.day < 1 || date.day > 31 {
                return Err(DatetimeParseError { _private: () });
            }

            Some(date)
        };

        // Next parse the "partial-time" if available
        let next = chars.clone().next();
        let partial_time = if full_date.is_some()
            && (next == Some('T') || next == Some('t') || next == Some(' '))
        {
            chars.next();
            true
        } else {
            full_date.is_none()
        };

        let time = if partial_time {
            let h1 = digit(&mut chars)?;
            let h2 = digit(&mut chars)?;
            match chars.next() {
                Some(':') => {}
                _ => return Err(DatetimeParseError { _private: () }),
            }
            let m1 = digit(&mut chars)?;
            let m2 = digit(&mut chars)?;
            match chars.next() {
                Some(':') => {}
                _ => return Err(DatetimeParseError { _private: () }),
            }
            let s1 = digit(&mut chars)?;
            let s2 = digit(&mut chars)?;

            let mut nanosecond = 0;
            if chars.clone().next() == Some('.') {
                chars.next();
                let whole = chars.as_str();

                let mut end = whole.len();
                for (i, byte) in whole.bytes().enumerate() {
                    match byte {
                        b'0'..=b'9' => {
                            if i < 9 {
                                let p = 10_u32.pow(8 - i as u32);
                                nanosecond += p * u32::from(byte - b'0');
                            }
                        }
                        _ => {
                            end = i;
                            break;
                        }
                    }
                }
                if end == 0 {
                    return Err(DatetimeParseError { _private: () });
                }
                chars = whole[end..].chars();
            }

            let time = Time {
                hour: h1 * 10 + h2,
                minute: m1 * 10 + m2,
                second: s1 * 10 + s2,
                nanosecond,
            };

            if time.hour > 24 {
                return Err(DatetimeParseError { _private: () });
            }
            if time.minute > 59 {
                return Err(DatetimeParseError { _private: () });
            }
            if time.second > 59 {
                return Err(DatetimeParseError { _private: () });
            }
            if time.nanosecond > 999_999_999 {
                return Err(DatetimeParseError { _private: () });
            }

            Some(time)
        } else {
            offset_allowed = false;
            None
        };

        // And finally, parse the offset
        let offset = if offset_allowed {
            let next = chars.clone().next();
            if next == Some('Z') || next == Some('z') {
                chars.next();
                Some(Offset::Z)
            } else if next.is_none() {
                None
            } else {
                let sign = match next {
                    Some('+') => 1,
                    Some('-') => -1,
                    _ => return Err(DatetimeParseError { _private: () }),
                };
                chars.next();
                let h1 = digit(&mut chars)? as i8;
                let h2 = digit(&mut chars)? as i8;
                match chars.next() {
                    Some(':') => {}
                    _ => return Err(DatetimeParseError { _private: () }),
                }
                let m1 = digit(&mut chars)?;
                let m2 = digit(&mut chars)?;

                Some(Offset::Custom {
                    hours: sign * (h1 * 10 + h2),
                    minutes: m1 * 10 + m2,
                })
            }
        } else {
            None
        };

        // Return an error if we didn't hit eof, otherwise return our parsed
        // date
        if chars.next().is_some() {
            return Err(DatetimeParseError { _private: () });
        }

        Ok(Datetime {
            date: full_date,
            time,
            offset,
        })
    }
}

fn digit(chars: &mut str::Chars<'_>) -> Result<u8, DatetimeParseError> {
    match chars.next() {
        Some(c) if '0' <= c && c <= '9' => Ok(c as u8 - b'0'),
        _ => Err(DatetimeParseError { _private: () }),
    }
}

impl ser::Serialize for Datetime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut s = serializer.serialize_struct(NAME, 1)?;
        s.serialize_field(FIELD, &self.to_string())?;
        s.end()
    }
}

impl<'de> de::Deserialize<'de> for Datetime {
    fn deserialize<D>(deserializer: D) -> Result<Datetime, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct DatetimeVisitor;

        impl<'de> de::Visitor<'de> for DatetimeVisitor {
            type Value = Datetime;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a TOML datetime")
            }

            fn visit_map<V>(self, mut visitor: V) -> Result<Datetime, V::Error>
            where
                V: de::MapAccess<'de>,
            {
                let value = visitor.next_key::<DatetimeKey>()?;
                if value.is_none() {
                    return Err(de::Error::custom("datetime key not found"));
                }
                let v: DatetimeFromString = visitor.next_value()?;
                Ok(v.value)
            }
        }

        static FIELDS: [&str; 1] = [FIELD];
        deserializer.deserialize_struct(NAME, &FIELDS, DatetimeVisitor)
    }
}

struct DatetimeKey;

impl<'de> de::Deserialize<'de> for DatetimeKey {
    fn deserialize<D>(deserializer: D) -> Result<DatetimeKey, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct FieldVisitor;

        impl<'de> de::Visitor<'de> for FieldVisitor {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a valid datetime field")
            }

            fn visit_str<E>(self, s: &str) -> Result<(), E>
            where
                E: de::Error,
            {
                if s == FIELD {
                    Ok(())
                } else {
                    Err(de::Error::custom("expected field with custom name"))
                }
            }
        }

        deserializer.deserialize_identifier(FieldVisitor)?;
        Ok(DatetimeKey)
    }
}

pub struct DatetimeFromString {
    pub value: Datetime,
}

impl<'de> de::Deserialize<'de> for DatetimeFromString {
    fn deserialize<D>(deserializer: D) -> Result<DatetimeFromString, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = DatetimeFromString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string containing a datetime")
            }

            fn visit_str<E>(self, s: &str) -> Result<DatetimeFromString, E>
            where
                E: de::Error,
            {
                match s.parse() {
                    Ok(date) => Ok(DatetimeFromString { value: date }),
                    Err(e) => Err(de::Error::custom(e)),
                }
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl fmt::Display for DatetimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "failed to parse datetime".fmt(f)
    }
}

impl error::Error for DatetimeParseError {}
