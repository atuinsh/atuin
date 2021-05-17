use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for [u8] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::BYTEA
    }
}

impl Type<Postgres> for Vec<u8> {
    fn type_info() -> PgTypeInfo {
        <[u8] as Type<Postgres>>::type_info()
    }
}

impl Type<Postgres> for [&'_ [u8]] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::BYTEA_ARRAY
    }
}

impl Type<Postgres> for [Vec<u8>] {
    fn type_info() -> PgTypeInfo {
        <[&[u8]] as Type<Postgres>>::type_info()
    }
}

impl Type<Postgres> for Vec<&'_ [u8]> {
    fn type_info() -> PgTypeInfo {
        <[&[u8]] as Type<Postgres>>::type_info()
    }
}

impl Type<Postgres> for Vec<Vec<u8>> {
    fn type_info() -> PgTypeInfo {
        <[&[u8]] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for &'_ [u8] {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend_from_slice(self);

        IsNull::No
    }
}

impl Encode<'_, Postgres> for Vec<u8> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        <&[u8] as Encode<Postgres>>::encode(self, buf)
    }
}

impl<'r> Decode<'r, Postgres> for &'r [u8] {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            PgValueFormat::Binary => value.as_bytes(),
            PgValueFormat::Text => {
                Err("unsupported decode to `&[u8]` of BYTEA in a simple query; use a prepared query or decode to `Vec<u8>`".into())
            }
        }
    }
}

impl Decode<'_, Postgres> for Vec<u8> {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => value.as_bytes()?.to_owned(),
            PgValueFormat::Text => {
                // BYTEA is formatted as \x followed by hex characters
                hex::decode(&value.as_str()?[2..])?
            }
        })
    }
}
