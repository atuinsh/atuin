use byteorder::{BigEndian, ByteOrder};

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for f32 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::FLOAT4
    }
}

impl Type<Postgres> for [f32] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::FLOAT4_ARRAY
    }
}

impl Type<Postgres> for Vec<f32> {
    fn type_info() -> PgTypeInfo {
        <[f32] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for f32 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for f32 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_f32(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}

impl Type<Postgres> for f64 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::FLOAT8
    }
}

impl Type<Postgres> for [f64] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::FLOAT8_ARRAY
    }
}

impl Type<Postgres> for Vec<f64> {
    fn type_info() -> PgTypeInfo {
        <[f64] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for f64 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for f64 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_f64(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}
