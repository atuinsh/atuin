use uuid::{adapter::Hyphenated, Uuid};

use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::mysql::io::MySqlBufMutExt;
use crate::mysql::{MySql, MySqlTypeInfo, MySqlValueRef};
use crate::types::Type;

impl Type<MySql> for Uuid {
    fn type_info() -> MySqlTypeInfo {
        <&[u8] as Type<MySql>>::type_info()
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        <&[u8] as Type<MySql>>::compatible(ty)
    }
}

impl Encode<'_, MySql> for Uuid {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.put_bytes_lenenc(self.as_bytes());

        IsNull::No
    }
}

impl Decode<'_, MySql> for Uuid {
    fn decode(value: MySqlValueRef<'_>) -> Result<Self, BoxDynError> {
        // delegate to the &[u8] type to decode from MySQL
        let bytes = <&[u8] as Decode<MySql>>::decode(value)?;

        // construct a Uuid from the returned bytes
        Uuid::from_slice(bytes).map_err(Into::into)
    }
}

impl Type<MySql> for Hyphenated {
    fn type_info() -> MySqlTypeInfo {
        <&str as Type<MySql>>::type_info()
    }

    fn compatible(ty: &MySqlTypeInfo) -> bool {
        <&str as Type<MySql>>::compatible(ty)
    }
}

impl Encode<'_, MySql> for Hyphenated {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.put_str_lenenc(&self.to_string());

        IsNull::No
    }
}

impl Decode<'_, MySql> for Hyphenated {
    fn decode(value: MySqlValueRef<'_>) -> Result<Self, BoxDynError> {
        // delegate to the &str type to decode from MySQL
        let text = <&str as Decode<MySql>>::decode(value)?;

        // parse a UUID from the text
        Uuid::parse_str(text)
            .map_err(Into::into)
            .map(|u| u.to_hyphenated())
    }
}
