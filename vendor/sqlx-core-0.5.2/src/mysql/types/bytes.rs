use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::mysql::io::MySqlBufMutExt;
use crate::mysql::protocol::text::ColumnType;
use crate::mysql::{MySql, MySqlTypeInfo, MySqlValueRef};
use crate::types::Type;

impl Type<MySql> for [u8] {
    fn type_info() -> MySqlTypeInfo {
        MySqlTypeInfo::binary(ColumnType::Blob)
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        matches!(
            ty.r#type,
            ColumnType::VarChar
                | ColumnType::Blob
                | ColumnType::TinyBlob
                | ColumnType::MediumBlob
                | ColumnType::LongBlob
                | ColumnType::String
                | ColumnType::VarString
                | ColumnType::Enum
        )
    }
}

impl Encode<'_, MySql> for &'_ [u8] {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.put_bytes_lenenc(self);

        IsNull::No
    }
}

impl<'r> Decode<'r, MySql> for &'r [u8] {
    fn decode(value: MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        value.as_bytes()
    }
}

impl Type<MySql> for Vec<u8> {
    fn type_info() -> MySqlTypeInfo {
        <[u8] as Type<MySql>>::type_info()
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        <&[u8] as Type<MySql>>::compatible(ty)
    }
}

impl Encode<'_, MySql> for Vec<u8> {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        <&[u8] as Encode<MySql>>::encode(&**self, buf)
    }
}

impl Decode<'_, MySql> for Vec<u8> {
    fn decode(value: MySqlValueRef<'_>) -> Result<Self, BoxDynError> {
        <&[u8] as Decode<MySql>>::decode(value).map(ToOwned::to_owned)
    }
}
