use crate::error::{BoxDynError, UnexpectedNullError};
use crate::postgres::{PgTypeInfo, Postgres};
use crate::value::{Value, ValueRef};
use bytes::{Buf, Bytes};
use std::borrow::Cow;
use std::str::from_utf8;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[repr(u8)]
pub enum PgValueFormat {
    Text = 0,
    Binary = 1,
}

/// Implementation of [`ValueRef`] for PostgreSQL.
#[derive(Clone)]
pub struct PgValueRef<'r> {
    pub(crate) value: Option<&'r [u8]>,
    pub(crate) row: Option<&'r Bytes>,
    pub(crate) type_info: PgTypeInfo,
    pub(crate) format: PgValueFormat,
}

/// Implementation of [`Value`] for PostgreSQL.
#[derive(Clone)]
pub struct PgValue {
    pub(crate) value: Option<Bytes>,
    pub(crate) type_info: PgTypeInfo,
    pub(crate) format: PgValueFormat,
}

impl<'r> PgValueRef<'r> {
    pub(crate) fn get(buf: &mut &'r [u8], format: PgValueFormat, ty: PgTypeInfo) -> Self {
        let mut element_len = buf.get_i32();

        let element_val = if element_len == -1 {
            element_len = 0;
            None
        } else {
            Some(&buf[..(element_len as usize)])
        };

        buf.advance(element_len as usize);

        PgValueRef {
            value: element_val,
            row: None,
            type_info: ty,
            format,
        }
    }

    pub(crate) fn format(&self) -> PgValueFormat {
        self.format
    }

    pub(crate) fn as_bytes(&self) -> Result<&'r [u8], BoxDynError> {
        match &self.value {
            Some(v) => Ok(v),
            None => Err(UnexpectedNullError.into()),
        }
    }

    pub(crate) fn as_str(&self) -> Result<&'r str, BoxDynError> {
        Ok(from_utf8(self.as_bytes()?)?)
    }
}

impl Value for PgValue {
    type Database = Postgres;

    #[inline]
    fn as_ref(&self) -> PgValueRef<'_> {
        PgValueRef {
            value: self.value.as_deref(),
            row: None,
            type_info: self.type_info.clone(),
            format: self.format,
        }
    }

    fn type_info(&self) -> Cow<'_, PgTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        self.value.is_none()
    }
}

impl<'r> ValueRef<'r> for PgValueRef<'r> {
    type Database = Postgres;

    fn to_owned(&self) -> PgValue {
        let value = match (self.row, self.value) {
            (Some(row), Some(value)) => Some(row.slice_ref(value)),

            (None, Some(value)) => Some(Bytes::copy_from_slice(value)),

            _ => None,
        };

        PgValue {
            value,
            format: self.format,
            type_info: self.type_info.clone(),
        }
    }

    fn type_info(&self) -> Cow<'_, PgTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        self.value.is_none()
    }
}

#[cfg(feature = "any")]
impl<'r> From<PgValueRef<'r>> for crate::any::AnyValueRef<'r> {
    #[inline]
    fn from(value: PgValueRef<'r>) -> Self {
        crate::any::AnyValueRef {
            type_info: value.type_info.clone().into(),
            kind: crate::any::value::AnyValueRefKind::Postgres(value),
        }
    }
}

#[cfg(feature = "any")]
impl From<PgValue> for crate::any::AnyValue {
    #[inline]
    fn from(value: PgValue) -> Self {
        crate::any::AnyValue {
            type_info: value.type_info.clone().into(),
            kind: crate::any::value::AnyValueKind::Postgres(value),
        }
    }
}
