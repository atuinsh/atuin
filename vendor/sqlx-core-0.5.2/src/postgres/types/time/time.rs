use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;
use std::borrow::Cow;
use std::mem;
use time::{Duration, Time};

impl Type<Postgres> for Time {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIME
    }
}

impl Type<Postgres> for [Time] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TIME_ARRAY
    }
}

impl Type<Postgres> for Vec<Time> {
    fn type_info() -> PgTypeInfo {
        <[Time] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for Time {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        // TIME is encoded as the microseconds since midnight
        let us = (*self - Time::midnight()).whole_microseconds() as i64;
        Encode::<Postgres>::encode(&us, buf)
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<u64>()
    }
}

impl<'r> Decode<'r, Postgres> for Time {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => {
                // TIME is encoded as the microseconds since midnight
                let us = Decode::<Postgres>::decode(value)?;
                Time::midnight() + Duration::microseconds(us)
            }

            PgValueFormat::Text => {
                // If there are less than 9 digits after the decimal point
                // We need to zero-pad

                // FIXME: Ask [time] to add a parse % for less-than-fixed-9 nanos

                let s = value.as_str()?;

                let s = if s.len() < 20 {
                    Cow::Owned(format!("{:0<19}", s))
                } else {
                    Cow::Borrowed(s)
                };

                Time::parse(&*s, "%H:%M:%S.%N")?
            }
        })
    }
}
