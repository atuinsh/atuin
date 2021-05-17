use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::types::array_compatible;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef, Postgres};
use crate::types::Type;

impl Type<Postgres> for str {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TEXT
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        [
            PgTypeInfo::TEXT,
            PgTypeInfo::NAME,
            PgTypeInfo::BPCHAR,
            PgTypeInfo::VARCHAR,
            PgTypeInfo::UNKNOWN,
        ]
        .contains(ty)
    }
}

impl Type<Postgres> for [&'_ str] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::TEXT_ARRAY
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        array_compatible::<&str>(ty)
    }
}

impl Type<Postgres> for Vec<&'_ str> {
    fn type_info() -> PgTypeInfo {
        <[&str] as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <[&str] as Type<Postgres>>::compatible(ty)
    }
}

impl Encode<'_, Postgres> for &'_ str {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(self.as_bytes());

        IsNull::No
    }
}

impl Encode<'_, Postgres> for String {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        <&str as Encode<Postgres>>::encode(&**self, buf)
    }
}

impl<'r> Decode<'r, Postgres> for &'r str {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.as_str()?)
    }
}

impl Type<Postgres> for String {
    fn type_info() -> PgTypeInfo {
        <&str as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as Type<Postgres>>::compatible(ty)
    }
}

impl Type<Postgres> for [String] {
    fn type_info() -> PgTypeInfo {
        <[&str] as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <[&str] as Type<Postgres>>::compatible(ty)
    }
}

impl Type<Postgres> for Vec<String> {
    fn type_info() -> PgTypeInfo {
        <[String] as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <[String] as Type<Postgres>>::compatible(ty)
    }
}

impl Decode<'_, Postgres> for String {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        Ok(value.as_str()?.to_owned())
    }
}
