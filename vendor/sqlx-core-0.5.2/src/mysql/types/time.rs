use std::borrow::Cow;
use std::convert::TryFrom;

use byteorder::{ByteOrder, LittleEndian};
use bytes::Buf;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::{BoxDynError, UnexpectedNullError};
use crate::mysql::protocol::text::ColumnType;
use crate::mysql::type_info::MySqlTypeInfo;
use crate::mysql::{MySql, MySqlValueFormat, MySqlValueRef};
use crate::types::Type;

impl Type<MySql> for OffsetDateTime {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Timestamp)
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        matches!(ty.r#type, ColumnType::Datetime | ColumnType::Timestamp)
    }
}

impl Encode<'_, MySql> for OffsetDateTime {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        let utc_dt = self.to_offset(UtcOffset::UTC);
        let primitive_dt = PrimitiveDateTime::new(utc_dt.date(), utc_dt.time());

        Encode::<MySql>::encode(&primitive_dt, buf)
    }
}

impl<'r> Decode<'r, MySql> for OffsetDateTime {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        let primitive: PrimitiveDateTime = Decode::<MySql>::decode(value)?;

        Ok(primitive.assume_utc())
    }
}

impl Type<MySql> for Time {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Time)
    }
}

impl Encode<'_, MySql> for Time {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        let len = Encode::<MySql>::size_hint(self) - 1;
        buf.push(len as u8);

        // Time is not negative
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

impl<'r> Decode<'r, MySql> for Time {
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
                    return Ok(Time::try_from_hms_micro(0, 0, 0, 0).unwrap());
                }

                // is negative : int<1>
                let is_negative = buf.get_u8();
                assert_eq!(is_negative, 0, "Negative dates/times are not supported");

                // "date on 4 bytes little-endian format" (?)
                // https://mariadb.com/kb/en/resultset-row/#timestamp-binary-encoding
                buf.advance(4);

                decode_time(len - 5, buf)
            }

            MySqlValueFormat::Text => {
                let s = value.as_str()?;

                // If there are less than 9 digits after the decimal point
                // We need to zero-pad
                // TODO: Ask [time] to add a parse % for less-than-fixed-9 nanos

                let s = if s.len() < 20 {
                    Cow::Owned(format!("{:0<19}", s))
                } else {
                    Cow::Borrowed(s)
                };

                Time::parse(&*s, "%H:%M:%S.%N").map_err(Into::into)
            }
        }
    }
}

impl Type<MySql> for Date {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Date)
    }
}

impl Encode<'_, MySql> for Date {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.push(4);

        encode_date(self, buf);

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        5
    }
}

impl<'r> Decode<'r, MySql> for Date {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            MySqlValueFormat::Binary => {
                Ok(decode_date(&value.as_bytes()?[1..])?.ok_or(UnexpectedNullError)?)
            }
            MySqlValueFormat::Text => {
                let s = value.as_str()?;
                Date::parse(s, "%Y-%m-%d").map_err(Into::into)
            }
        }
    }
}

impl Type<MySql> for PrimitiveDateTime {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Datetime)
    }
}

impl Encode<'_, MySql> for PrimitiveDateTime {
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
        match (self.hour(), self.minute(), self.second(), self.nanosecond()) {
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

impl<'r> Decode<'r, MySql> for PrimitiveDateTime {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            MySqlValueFormat::Binary => {
                let buf = value.as_bytes()?;
                let len = buf[0];
                let date = decode_date(&buf[1..])?.ok_or(UnexpectedNullError)?;

                let dt = if len > 4 {
                    date.with_time(decode_time(len - 4, &buf[5..])?)
                } else {
                    date.midnight()
                };

                Ok(dt)
            }

            MySqlValueFormat::Text => {
                let s = value.as_str()?;

                // If there are less than 9 digits after the decimal point
                // We need to zero-pad
                // TODO: Ask [time] to add a parse % for less-than-fixed-9 nanos

                let s = if s.len() < 31 {
                    if s.contains('.') {
                        Cow::Owned(format!("{:0<30}", s))
                    } else {
                        Cow::Owned(format!("{}.000000000", s))
                    }
                } else {
                    Cow::Borrowed(s)
                };

                PrimitiveDateTime::parse(&*s, "%Y-%m-%d %H:%M:%S.%N").map_err(Into::into)
            }
        }
    }
}

fn encode_date(date: &Date, buf: &mut Vec<u8>) {
    // MySQL supports years from 1000 - 9999
    let year = u16::try_from(date.year())
        .unwrap_or_else(|_| panic!("Date out of range for Mysql: {}", date));

    buf.extend_from_slice(&year.to_le_bytes());
    buf.push(date.month());
    buf.push(date.day());
}

fn decode_date(buf: &[u8]) -> Result<Option<Date>, BoxDynError> {
    if buf.is_empty() {
        // zero buffer means a zero date (null)
        return Ok(None);
    }

    Date::try_from_ymd(
        LittleEndian::read_u16(buf) as i32,
        buf[2] as u8,
        buf[3] as u8,
    )
    .map_err(Into::into)
    .map(Some)
}

fn encode_time(time: &Time, include_micros: bool, buf: &mut Vec<u8>) {
    buf.push(time.hour());
    buf.push(time.minute());
    buf.push(time.second());

    if include_micros {
        buf.extend(&((time.nanosecond() / 1000) as u32).to_le_bytes());
    }
}

fn decode_time(len: u8, mut buf: &[u8]) -> Result<Time, BoxDynError> {
    let hour = buf.get_u8();
    let minute = buf.get_u8();
    let seconds = buf.get_u8();

    let micros = if len > 3 {
        // microseconds : int<EOF>
        buf.get_uint_le(buf.len())
    } else {
        0
    };

    Time::try_from_hms_micro(hour, minute, seconds, micros as u32)
        .map_err(|e| format!("Time out of range for MySQL: {}", e).into())
}
