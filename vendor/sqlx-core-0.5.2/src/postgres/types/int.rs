use byteorder::{BigEndian, ByteOrder};

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for i8 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::CHAR
    }
}

impl Type<Postgres> for [i8] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::CHAR_ARRAY
    }
}

impl Type<Postgres> for Vec<i8> {
    fn type_info() -> PgTypeInfo {
        <[i8] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for i8 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for i8 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        // note: in the TEXT encoding, a value of "0" here is encoded as an empty string
        Ok(value.as_bytes()?.get(0).copied().unwrap_or_default() as i8)
    }
}

impl Type<Postgres> for i16 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT2
    }
}

impl Type<Postgres> for [i16] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT2_ARRAY
    }
}

impl Type<Postgres> for Vec<i16> {
    fn type_info() -> PgTypeInfo {
        <[i16] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for i16 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for i16 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_i16(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}

impl Type<Postgres> for u32 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::OID
    }
}

impl Type<Postgres> for [u32] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::OID_ARRAY
    }
}

impl Type<Postgres> for Vec<u32> {
    fn type_info() -> PgTypeInfo {
        <[u32] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for u32 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for u32 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_u32(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}

impl Type<Postgres> for i32 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT4
    }
}

impl Type<Postgres> for [i32] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT4_ARRAY
    }
}

impl Type<Postgres> for Vec<i32> {
    fn type_info() -> PgTypeInfo {
        <[i32] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for i32 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for i32 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_i32(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}

impl Type<Postgres> for i64 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT8
    }
}

impl Type<Postgres> for [i64] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::INT8_ARRAY
    }
}

impl Type<Postgres> for Vec<i64> {
    fn type_info() -> PgTypeInfo {
        <[i64] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for i64 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for i64 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => BigEndian::read_i64(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse()?,
        })
    }
}
