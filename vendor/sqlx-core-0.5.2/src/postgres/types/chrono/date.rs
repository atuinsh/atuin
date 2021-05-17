use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;
use chrono::{Duration, NaiveDate};
use std::mem;

impl Type<Postgres> for NaiveDate {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::DATE
    }
}

impl Type<Postgres> for [NaiveDate] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::DATE_ARRAY
    }
}

impl Type<Postgres> for Vec<NaiveDate> {
    fn type_info() -> PgTypeInfo {
        <[NaiveDate] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for NaiveDate {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        // DATE is encoded as the days since epoch
        let days = (*self - NaiveDate::from_ymd(2000, 1, 1)).num_days() as i32;
        Encode::<Postgres>::encode(&days, buf)
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<i32>()
    }
}

impl<'r> Decode<'r, Postgres> for NaiveDate {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => {
                // DATE is encoded as the days since epoch
                let days: i32 = Decode::<Postgres>::decode(value)?;
                NaiveDate::from_ymd(2000, 1, 1) + Duration::days(days.into())
            }

            PgValueFormat::Text => NaiveDate::parse_from_str(value.as_str()?, "%Y-%m-%d")?,
        })
    }
}
