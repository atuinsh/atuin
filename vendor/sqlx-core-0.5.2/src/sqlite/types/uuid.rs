use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::sqlite::type_info::DataType;
use crate::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use crate::types::Type;
use std::borrow::Cow;
use uuid::{adapter::Hyphenated, Uuid};

impl Type<Sqlite> for Uuid {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Blob)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Blob | DataType::Text)
    }
}

impl<'q> Encode<'q, Sqlite> for Uuid {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Blob(Cow::Owned(
            self.as_bytes().to_vec(),
        )));

        IsNull::No
    }
}

impl Decode<'_, Sqlite> for Uuid {
    fn decode(value: SqliteValueRef<'_>) -> Result<Self, BoxDynError> {
        // construct a Uuid from the returned bytes
        Uuid::from_slice(value.blob()).map_err(Into::into)
    }
}

impl Type<Sqlite> for Hyphenated {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Text)
    }
}

impl<'q> Encode<'q, Sqlite> for Hyphenated {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        args.push(SqliteArgumentValue::Text(Cow::Owned(self.to_string())));

        IsNull::No
    }
}

impl Decode<'_, Sqlite> for Hyphenated {
    fn decode(value: SqliteValueRef<'_>) -> Result<Self, BoxDynError> {
        let uuid: Result<Uuid, BoxDynError> =
            Uuid::parse_str(&value.text().map(ToOwned::to_owned)?).map_err(Into::into);

        Ok(uuid?.to_hyphenated())
    }
}
