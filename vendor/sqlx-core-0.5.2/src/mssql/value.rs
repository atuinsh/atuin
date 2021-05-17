use crate::error::{BoxDynError, UnexpectedNullError};
use crate::mssql::{Mssql, MssqlTypeInfo};
use crate::value::{Value, ValueRef};
use bytes::Bytes;
use std::borrow::Cow;

/// Implementation of [`ValueRef`] for MSSQL.
#[derive(Clone)]
pub struct MssqlValueRef<'r> {
    pub(crate) type_info: MssqlTypeInfo,
    pub(crate) data: Option<&'r Bytes>,
}

impl<'r> MssqlValueRef<'r> {
    pub(crate) fn as_bytes(&self) -> Result<&'r [u8], BoxDynError> {
        match &self.data {
            Some(v) => Ok(v),
            None => Err(UnexpectedNullError.into()),
        }
    }
}

impl ValueRef<'_> for MssqlValueRef<'_> {
    type Database = Mssql;

    fn to_owned(&self) -> MssqlValue {
        MssqlValue {
            data: self.data.cloned(),
            type_info: self.type_info.clone(),
        }
    }

    fn type_info(&self) -> Cow<'_, MssqlTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        self.data.is_none() || self.type_info.0.is_null()
    }
}

#[cfg(feature = "any")]
impl<'r> From<MssqlValueRef<'r>> for crate::any::AnyValueRef<'r> {
    #[inline]
    fn from(value: MssqlValueRef<'r>) -> Self {
        crate::any::AnyValueRef {
            type_info: value.type_info.clone().into(),
            kind: crate::any::value::AnyValueRefKind::Mssql(value),
        }
    }
}

/// Implementation of [`Value`] for MSSQL.
#[derive(Clone)]
pub struct MssqlValue {
    pub(crate) type_info: MssqlTypeInfo,
    pub(crate) data: Option<Bytes>,
}

impl Value for MssqlValue {
    type Database = Mssql;

    fn as_ref(&self) -> MssqlValueRef<'_> {
        MssqlValueRef {
            data: self.data.as_ref(),
            type_info: self.type_info.clone(),
        }
    }

    fn type_info(&self) -> Cow<'_, MssqlTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        self.data.is_none() || self.type_info.0.is_null()
    }
}

#[cfg(feature = "any")]
impl From<MssqlValue> for crate::any::AnyValue {
    #[inline]
    fn from(value: MssqlValue) -> Self {
        crate::any::AnyValue {
            type_info: value.type_info.clone().into(),
            kind: crate::any::value::AnyValueKind::Mssql(value),
        }
    }
}
