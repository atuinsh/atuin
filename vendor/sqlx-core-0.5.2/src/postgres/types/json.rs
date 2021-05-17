use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::postgres::types::array_compatible;
use crate::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres};
use crate::types::{Json, Type};
use serde::{Deserialize, Serialize};

// <https://www.postgresql.org/docs/12/datatype-json.html>

// In general, most applications should prefer to store JSON data as jsonb,
// unless there are quite specialized needs, such as legacy assumptions
// about ordering of object keys.

impl<T> Type<Postgres> for Json<T> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::JSONB
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        *ty == PgTypeInfo::JSON || *ty == PgTypeInfo::JSONB
    }
}

impl<T> Type<Postgres> for [Json<T>] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::JSONB_ARRAY
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        array_compatible::<Json<T>>(ty)
    }
}

impl<T> Type<Postgres> for Vec<Json<T>> {
    fn type_info() -> PgTypeInfo {
        <[Json<T>] as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <[Json<T>] as Type<Postgres>>::compatible(ty)
    }
}

impl<'q, T> Encode<'q, Postgres> for Json<T>
where
    T: Serialize,
{
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        // we have a tiny amount of dynamic behavior depending if we are resolved to be JSON
        // instead of JSONB
        buf.patch(|buf, ty: &PgTypeInfo| {
            if *ty == PgTypeInfo::JSON || *ty == PgTypeInfo::JSON_ARRAY {
                buf[0] = b' ';
            }
        });

        // JSONB version (as of 2020-03-20)
        buf.push(1);

        // the JSON data written to the buffer is the same regardless of parameter type
        serde_json::to_writer(&mut **buf, &self.0)
            .expect("failed to serialize to JSON for encoding on transmission to the database");

        IsNull::No
    }
}

impl<'r, T: 'r> Decode<'r, Postgres> for Json<T>
where
    T: Deserialize<'r>,
{
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let mut buf = value.as_bytes()?;

        if value.format() == PgValueFormat::Binary && value.type_info == PgTypeInfo::JSONB {
            assert_eq!(
                buf[0], 1,
                "unsupported JSONB format version {}; please open an issue",
                buf[0]
            );

            buf = &buf[1..];
        }

        serde_json::from_slice(buf).map(Json).map_err(Into::into)
    }
}
