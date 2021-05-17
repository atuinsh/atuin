use uuid::Uuid;

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for Uuid {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::UUID
    }
}

impl Type<Postgres> for [Uuid] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::UUID_ARRAY
    }
}

impl Type<Postgres> for Vec<Uuid> {
    fn type_info() -> PgTypeInfo {
        <[Uuid] as Type<Postgres>>::type_info()
    }
}

impl Encode<'_, Postgres> for Uuid {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend_from_slice(self.as_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for Uuid {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        match value.format() {
            PgValueFormat::Binary => Uuid::from_slice(value.as_bytes()?),
            PgValueFormat::Text => value.as_str()?.parse(),
        }
        .map_err(Into::into)
    }
}
