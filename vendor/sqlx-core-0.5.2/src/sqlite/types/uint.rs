use std::convert::TryInto;

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::sqlite::type_info::DataType;
use crate::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use crate::types::Type;

impl Type<Sqlite> for u8 {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Int)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Int | DataType::Int64)
    }
}

impl<'q> Encode<'q, Sqlite> for u8 {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Int(*self as i32));

        IsNull::No
    }
}

impl<'r> Decode<'r, Sqlite> for u8 {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.int().try_into()?)
    }
}

impl Type<Sqlite> for u16 {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Int)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Int | DataType::Int64)
    }
}

impl<'q> Encode<'q, Sqlite> for u16 {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Int(*self as i32));

        IsNull::No
    }
}

impl<'r> Decode<'r, Sqlite> for u16 {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.int().try_into()?)
    }
}

impl Type<Sqlite> for u32 {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Int64)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Int | DataType::Int64)
    }
}

impl<'q> Encode<'q, Sqlite> for u32 {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Int64(*self as i64));

        IsNull::No
    }
}

impl<'r> Decode<'r, Sqlite> for u32 {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.int64().try_into()?)
    }
}
