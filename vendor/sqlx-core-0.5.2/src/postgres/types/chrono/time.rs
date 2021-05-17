use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;
use chrono::{Duration, NaiveTime};
use std::mem;

impl Type<Postgres> for NaiveTime {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIME
    }
}

impl Type<Postgres> for [NaiveTime] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIME_ARRAY
    }
}

impl Type<Postgres> for Vec<NaiveTime> {
    fn type_info() -> PgTypeInfo {
        <[NaiveTime] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for NaiveTime {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        // TIME is encoded as the microseconds since midnight
        // NOTE: panic! is on overflow and 1 day does not have enough micros to overflow
        let us = (*self - NaiveTime::from_hms(0, 0, 0))
            .num_microseconds()
            .unwrap();

        Encode::<Postgres>::encode(&us, buf)
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<u64>()
    }
}

impl<'r> Decode<'r, Postgres> for NaiveTime {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => {
                // TIME is encoded as the microseconds since midnight
                let us: i64 = Decode::<Postgres>::decode(value)?;
                NaiveTime::from_hms(0, 0, 0) + Duration::microseconds(us)
            }

            PgValueFormat::Text => NaiveTime::parse_from_str(value.as_str()?, "%H:%M:%S%.f")?,
        })
    }
}
