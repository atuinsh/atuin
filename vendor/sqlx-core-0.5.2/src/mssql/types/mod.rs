use crate::encode::{Encode, IsNull};
use crate::mssql::protocol::type_info::{DataType, TypeInfo};
use crate::mssql::{Mssql, MssqlTypeInfo};

mod bool;
mod float;
mod int;
mod str;

impl<'q, T: 'q + Encode<'q, Mssql>> Encode<'q, Mssql> for Option<T> {
    fn encode(self, buf: &mut Vec<u8>) -> IsNull {
        if let Some(v) = self {
            v.encode(buf)
        } else {
            IsNull::Yes
        }
    }

    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        if let Some(v) = self {
            v.encode_by_ref(buf)
        } else {
            IsNull::Yes
        }
    }

    fn produces(&self) -> Option<MssqlTypeInfo> {
        if let Some(v) = self {
            v.produces()
        } else {
            // MSSQL requires a special NULL type ID
            Some(MssqlTypeInfo(TypeInfo::new(DataType::Null, 0)))
        }
    }

    fn size_hint(&self) -> usize {
        self.as_ref().map_or(0, Encode::size_hint)
    }
}
