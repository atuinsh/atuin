use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;
use chrono::{
    DateTime, Duration, FixedOffset, Local, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc,
};
use std::mem;

impl Type<Postgres> for NaiveDateTime {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIMESTAMP
    }
}

impl<Tz: TimeZone> Type<Postgres> for DateTime<Tz> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIMESTAMPTZ
    }
}

impl Type<Postgres> for [NaiveDateTime] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIMESTAMP_ARRAY
    }
}

impl<Tz: TimeZone> Type<Postgres> for [DateTime<Tz>] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIMESTAMPTZ_ARRAY
    }
}

impl Type<Postgres> for Vec<NaiveDateTime> {
    fn type_info() -> PgTypeInfo {
        <[NaiveDateTime] as Type<Postgres>>::type_info()
    }
}

impl<Tz: TimeZone> Type<Postgres> for Vec<DateTime<Tz>> {
    fn type_info() -> PgTypeInfo {
        <[DateTime<Tz>] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for NaiveDateTime {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        // FIXME: We should *really* be returning an error, Encode needs to be fallible
        // TIMESTAMP is encoded as the microseconds since the epoch
        let epoch = NaiveDate::from_ymd(2000, 1, 1).and_hms(0, 0, 0);
        let us = (*self - epoch)
            .num_microseconds()
            .unwrap_or_else(|| panic!("NaiveDateTime out of range for Postgres: {:?}", self));

        Encode::<Postgres>::encode(&us, buf)
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<i64>()
    }
}

impl<'r> Decode<'r, Postgres> for NaiveDateTime {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => {
                // TIMESTAMP is encoded as the microseconds since the epoch
                let epoch = NaiveDate::from_ymd(2000, 1, 1).and_hms(0, 0, 0);
                let us = Decode::<Postgres>::decode(value)?;
                epoch + Duration::microseconds(us)
            }

            PgValueFormat::Text => {
                let s = value.as_str()?;
                NaiveDateTime::parse_from_str(
                    s,
                    if s.contains('+') {
                        // Contains a time-zone specifier
                        // This is given for timestamptz for some reason
                        // Postgres already guarantees this to always be UTC
                        "%Y-%m-%d %H:%M:%S%.f%#z"
                    } else {
                        "%Y-%m-%d %H:%M:%S%.f"
                    },
                )?
            }
        })
    }
}

impl<Tz: TimeZone> Encode<'_, Postgres> for DateTime<Tz> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        Encode::<Postgres>::encode(self.naive_utc(), buf)
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<i64>()
    }
}

impl<'r> Decode<'r, Postgres> for DateTime<Local> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let naive = <NaiveDateTime as Decode<Postgres>>::decode(value)?;
        Ok(Local.from_utc_datetime(&naive))
    }
}

impl<'r> Decode<'r, Postgres> for DateTime<Utc> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let naive = <NaiveDateTime as Decode<Postgres>>::decode(value)?;
        Ok(Utc.from_utc_datetime(&naive))
    }
}

impl<'r> Decode<'r, Postgres> for DateTime<FixedOffset> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let naive = <NaiveDateTime as Decode<Postgres>>::decode(value)?;
        Ok(Utc.fix().from_utc_datetime(&naive))
    }
}
