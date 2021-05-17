use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::sqlite::type_info::DataType;
use crate::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use crate::types::Type;

impl Type<Sqlite> for bool {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Bool)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Bool | DataType::Int | DataType::Int64)
    }
}

impl<'q> Encode<'q, Sqlite> for bool {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Int((*self).into()));

        IsNull::No
    }
}

impl<'r> Decode<'r, Sqlite> for bool {
    fn decode(value: SqliteValueRef<'r>) -> Result<bool, BoxDynError> {
        Ok(value.int() != 0)
    }
}
