use std::ptr::NonNull;
use std::slice::from_raw_parts;
use std::str::from_utf8;
use std::sync::Arc;

use libsqlite3_sys::{
    sqlite3_value, sqlite3_value_blob, sqlite3_value_bytes, sqlite3_value_double,
    sqlite3_value_dup, sqlite3_value_free, sqlite3_value_int, sqlite3_value_int64,
    sqlite3_value_type, SQLITE_NULL,
};

use crate::error::BoxDynError;
use crate::sqlite::statement::StatementHandle;
use crate::sqlite::type_info::DataType;
use crate::sqlite::{Sqlite, SqliteTypeInfo};
use crate::value::{Value, ValueRef};
use std::borrow::Cow;

enum SqliteValueData<'r> {
    Statement {
        statement: &'r StatementHandle,
        type_info: SqliteTypeInfo,
        index: usize,
    },

    Value(&'r SqliteValue),
}

pub struct SqliteValueRef<'r>(SqliteValueData<'r>);

impl<'r> SqliteValueRef<'r> {
    pub(crate) fn value(value: &'r SqliteValue) -> Self {
        Self(SqliteValueData::Value(value))
    }

    pub(crate) fn statement(
        statement: &'r StatementHandle,
        type_info: SqliteTypeInfo,
        index: usize,
    ) -> Self {
        Self(SqliteValueData::Statement {
            statement,
            type_info,
            index,
        })
    }

    pub(super) fn int(&self) -> i32 {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_int(index),

            SqliteValueData::Value(v) => v.int(),
        }
    }

    pub(super) fn int64(&self) -> i64 {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_int64(index),

            SqliteValueData::Value(v) => v.int64(),
        }
    }

    pub(super) fn double(&self) -> f64 {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_double(index),

            SqliteValueData::Value(v) => v.double(),
        }
    }

    pub(super) fn blob(&self) -> &'r [u8] {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_blob(index),

            SqliteValueData::Value(v) => v.blob(),
        }
    }

    pub(super) fn text(&self) -> Result<&'r str, BoxDynError> {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_text(index),

            SqliteValueData::Value(v) => v.text(),
        }
    }
}

impl<'r> ValueRef<'r> for SqliteValueRef<'r> {
    type Database = Sqlite;

    fn to_owned(&self) -> SqliteValue {
        match self.0 {
            SqliteValueData::Statement {
                statement,
                index,
                ref type_info,
            } => unsafe { SqliteValue::new(statement.column_value(index), type_info.clone()) },

            SqliteValueData::Value(v) => v.clone(),
        }
    }

    fn type_info(&self) -> Cow<'_, SqliteTypeInfo> {
        match self.0 {
            SqliteValueData::Value(v) => v.type_info(),

            SqliteValueData::Statement {
                ref type_info,
                statement,
                index,
            } => statement
                .column_type_info_opt(index)
                .map(Cow::Owned)
                .unwrap_or(Cow::Borrowed(type_info)),
        }
    }

    fn is_null(&self) -> bool {
        match self.0 {
            SqliteValueData::Statement {
                statement, index, ..
            } => statement.column_type(index) == SQLITE_NULL,

            SqliteValueData::Value(v) => v.is_null(),
        }
    }
}

#[derive(Clone)]
pub struct SqliteValue {
    pub(crate) handle: Arc<ValueHandle>,
    pub(crate) type_info: SqliteTypeInfo,
}

pub(crate) struct ValueHandle(NonNull<sqlite3_value>);

// SAFE: only protected value objects are stored in SqliteValue
unsafe impl Send for ValueHandle {}
unsafe impl Sync for ValueHandle {}

impl SqliteValue {
    pub(crate) unsafe fn new(value: *mut sqlite3_value, type_info: SqliteTypeInfo) -> Self {
        debug_assert!(!value.is_null());

        Self {
            type_info,
            handle: Arc::new(ValueHandle(NonNull::new_unchecked(sqlite3_value_dup(
                value,
            )))),
        }
    }

    fn type_info_opt(&self) -> Option<SqliteTypeInfo> {
        let dt = DataType::from_code(unsafe { sqlite3_value_type(self.handle.0.as_ptr()) });

        if let DataType::Null = dt {
            None
        } else {
            Some(SqliteTypeInfo(dt))
        }
    }

    fn int(&self) -> i32 {
        unsafe { sqlite3_value_int(self.handle.0.as_ptr()) }
    }

    fn int64(&self) -> i64 {
        unsafe { sqlite3_value_int64(self.handle.0.as_ptr()) }
    }

    fn double(&self) -> f64 {
        unsafe { sqlite3_value_double(self.handle.0.as_ptr()) }
    }

    fn blob(&self) -> &[u8] {
        let len = unsafe { sqlite3_value_bytes(self.handle.0.as_ptr()) } as usize;

        if len == 0 {
            // empty blobs are NULL so just return an empty slice
            return &[];
        }

        let ptr = unsafe { sqlite3_value_blob(self.handle.0.as_ptr()) } as *const u8;
        debug_assert!(!ptr.is_null());

        unsafe { from_raw_parts(ptr, len) }
    }

    fn text(&self) -> Result<&str, BoxDynError> {
        Ok(from_utf8(self.blob())?)
    }
}

impl Value for SqliteValue {
    type Database = Sqlite;

    fn as_ref(&self) -> SqliteValueRef<'_> {
        SqliteValueRef::value(self)
    }

    fn type_info(&self) -> Cow<'_, SqliteTypeInfo> {
        self.type_info_opt()
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(&self.type_info))
    }

    fn is_null(&self) -> bool {
        unsafe { sqlite3_value_type(self.handle.0.as_ptr()) == SQLITE_NULL }
    }
}

impl Drop for ValueHandle {
    fn drop(&mut self) {
        unsafe {
            sqlite3_value_free(self.0.as_ptr());
        }
    }
}

#[cfg(feature = "any")]
impl<'r> From<SqliteValueRef<'r>> for crate::any::AnyValueRef<'r> {
    #[inline]
    fn from(value: SqliteValueRef<'r>) -> Self {
        crate::any::AnyValueRef {
            type_info: value.type_info().clone().into_owned().into(),
            kind: crate::any::value::AnyValueRefKind::Sqlite(value),
        }
    }
}

#[cfg(feature = "any")]
impl From<SqliteValue> for crate::any::AnyValue {
    #[inline]
    fn from(value: SqliteValue) -> Self {
        crate::any::AnyValue {
            type_info: value.type_info().clone().into_owned().into(),
            kind: crate::any::value::AnyValueKind::Sqlite(value),
        }
    }
}
