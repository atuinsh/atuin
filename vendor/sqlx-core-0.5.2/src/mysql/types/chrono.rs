use std::convert::TryFrom;

use bytes::Buf;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::{BoxDynError, UnexpectedNullError};
use crate::mysql::protocol::text::ColumnType;
use crate::mysql::type_info::MySqlTypeInfo;
use crate::mysql::{MySql, MySqlValueFormat, MySqlValueRef};
use crate::types::Type;

impl Type<MySql> for DateTime<Utc> {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Timestamp)
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        matches!(ty.r#type, ColumnType::Datetime | ColumnType::Timestamp)
    }
}

impl Encode<'_, MySql> for DateTime<Utc> {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        Encode::<MySql>::encode(&self.naive_utc(), buf)
    }
}

impl<'r> Decode<'r, MySql> for DateTime<Utc> {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        let naive: NaiveDateTime = Decode::<MySql>::decode(value)?;

        Ok(DateTime::from_utc(naive, Utc))
    }
}

impl Type<MySql> for NaiveTime {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Time)
    }
}

impl Encode<'_, MySql> for NaiveTime {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        let len = Encode::<MySql>::size_hint(self) - 1;
        buf.push(len as u8);

        // NaiveTime is not negative
        buf.push(0);

        // "date on 4 bytes little-endian format" (?)
        // https://mariadb.com/kb/en/resultset-row/#teimstamp-binary-encoding
        buf.extend_from_slice(&[0_u8; 4]);

        encode_time(self, len > 9, buf);

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        if self.nanosecond() == 0 {
            // if micro_seconds is 0, length is 8 and micro_seconds is not sent
            9
        } else {
            // otherwise length is 12
            13
        }
    }
}

impl<'r> Decode<'r, MySql> for NaiveTime {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            MySqlValueFormat::Binary => {
                let mut buf = value.as_bytes()?;

                // data length, expecting 8 or 12 (fractional seconds)
                let len = buf.get_u8();

                // MySQL specifies that if all of hours, minutes, seconds, microseconds
                // are 0 then the length is 0 and no further data is send
                // https://dev.mysql.com/doc/internals/en/binary-protocol-value.html
                if len == 0 {
                    return Ok(NaiveTime::from_hms_micro(0, 0, 0, 0));
                }

                // is negative : int<1>
                let is_negative = buf.get_u8();
                debug_assert_eq!(is_negative, 0, "Negative dates/times are not supported");

                // "date on 4 bytes little-endian format" (?)
                // https://mariadb.com/kb/en/resultset-row/#timestamp-binary-encoding
                buf.advance(4);

                Ok(decode_time(len - 5, buf))
            }

            MySqlValueFormat::Text => {
                let s = value.as_str()?;
                NaiveTime::parse_from_str(s, "%H:%M:%S%.f").map_err(Into::into)
            }
        }
    }
}

impl Type<MySql> for NaiveDate {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Date)
    }
}

impl Encode<'_, MySql> for NaiveDate {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.push(4);

        encode_date(self, buf);

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        5
    }
}

impl<'r> Decode<'r, MySql> for NaiveDate {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            MySqlValueFormat::Binary => {
                decode_date(&value.as_bytes()?[1..]).ok_or_else(|| UnexpectedNullError.into())
            }

            MySqlValueFormat::Text => {
                let s = value.as_str()?;
                NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(Into::into)
            }
        }
    }
}

impl Type<MySql> for NaiveDateTime {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Datetime)
    }
}

impl Encode<'_, MySql> for NaiveDateTime {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        let len = Encode::<MySql>::size_hint(self) - 1;
        buf.push(len as u8);

        encode_date(&self.date(), buf);

        if len > 4 {
            encode_time(&self.time(), len > 8, buf);
        }

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        // to save space the packet can be compressed:
        match (
            self.hour(),
            self.minute(),
            self.second(),
            self.timestamp_subsec_nanos(),
        ) {
            // if hour, minutes, seconds and micro_seconds are all 0,
            // length is 4 and no other field is sent
            (0, 0, 0, 0) => 5,

            // if micro_seconds is 0, length is 7
            // and micro_seconds is not sent
            (_, _, _, 0) => 8,

            // otherwise length is 11
            (_, _, _, _) => 12,
        }
    }
}

impl<'r> Decode<'r, MySql> for NaiveDateTime {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            MySqlValueFormat::Binary => {
                let buf = value.as_bytes()?;

                let len = buf[0];
                let date = decode_date(&buf[1..]).ok_or(UnexpectedNullError)?;

                let dt = if len > 4 {
                    date.and_time(decode_time(len - 4, &buf[5..]))
                } else {
                    date.and_hms(0, 0, 0)
                };

                Ok(dt)
            }

            MySqlValueFormat::Text => {
                let s = value.as_str()?;
                NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").map_err(Into::into)
            }
        }
    }
}

fn encode_date(date: &NaiveDate, buf: &mut Vec<u8>) {
    // MySQL supports years from 1000 - 9999
    let year = u16::try_from(date.year())
        .unwrap_or_else(|_| panic!("NaiveDateTime out of range for Mysql: {}", date));

    buf.extend_from_slice(&year.to_le_bytes());
    buf.push(date.month() as u8);
    buf.push(date.day() as u8);
}

fn decode_date(mut buf: &[u8]) -> Option<NaiveDate> {
    if buf.len() == 0 {
        // MySQL specifies that if there are no bytes, this is all zeros
        None
    } else {
        let year = buf.get_u16_le();
        Some(NaiveDate::from_ymd(
            year as i32,
            buf[0] as u32,
            buf[1] as u32,
        ))
    }
}

fn encode_time(time: &NaiveTime, include_micros: bool, buf: &mut Vec<u8>) {
    buf.push(time.hour() as u8);
    buf.push(time.minute() as u8);
    buf.push(time.second() as u8);

    if include_micros {
        buf.extend(&((time.nanosecond() / 1000) as u32).to_le_bytes());
    }
}

fn decode_time(len: u8, mut buf: &[u8]) -> NaiveTime {
    let hour = buf.get_u8();
    let minute = buf.get_u8();
    let seconds = buf.get_u8();

    let micros = if len > 3 {
        // microseconds : int<EOF>
        buf.get_uint_le(buf.len())
    } else {
        0
    };

    NaiveTime::from_hms_micro(hour as u32, minute as u32, seconds as u32, micros as u32)
}
