use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for bool {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::BOOL
    }
}

impl Type<Postgres> for [bool] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::BOOL_ARRAY
    }
}

impl Type<Postgres> for Vec<bool> {
    fn type_info() -> PgTypeInfo {
        <[bool] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for bool {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.push(*self as u8);

        IsNull::No
    }
}

impl Decode<'_, Postgres> for bool {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(match value.format() {
            PgValueFormat::Binary => value.as_bytes()?[0] != 0,

            PgValueFormat::Text => match value.as_str()? {
                "t" => true,
                "f" => false,

                s => {
                    return Err(format!("unexpected value {:?} for boolean", s).into());
                }
            },
        })
    }
}
