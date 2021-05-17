use crate::any::{Any, AnyTypeInfo};
use crate::database::HasValueRef;
use crate::value::{Value, ValueRef};
use std::borrow::Cow;

#[cfg(feature = "postgres")]
use crate::postgres::{PgValue, PgValueRef};

#[cfg(feature = "mysql")]
use crate::mysql::{MySqlValue, MySqlValueRef};

#[cfg(feature = "sqlite")]
use crate::sqlite::{SqliteValue, SqliteValueRef};

#[cfg(feature = "mssql")]
use crate::mssql::{MssqlValue, MssqlValueRef};

pub struct AnyValue {
    pub(crate) kind: AnyValueKind,
    pub(crate) type_info: AnyTypeInfo,
}

pub(crate) enum AnyValueKind {
    #[cfg(feature = "postgres")]
    Postgres(PgValue),

    #[cfg(feature = "mysql")]
    MySql(MySqlValue),

    #[cfg(feature = "sqlite")]
    Sqlite(SqliteValue),

    #[cfg(feature = "mssql")]
    Mssql(MssqlValue),
}

pub struct AnyValueRef<'r> {
    pub(crate) kind: AnyValueRefKind<'r>,
    pub(crate) type_info: AnyTypeInfo,
}

pub(crate) enum AnyValueRefKind<'r> {
    #[cfg(feature = "postgres")]
    Postgres(PgValueRef<'r>),

    #[cfg(feature = "mysql")]
    MySql(MySqlValueRef<'r>),

    #[cfg(feature = "sqlite")]
    Sqlite(SqliteValueRef<'r>),

    #[cfg(feature = "mssql")]
    Mssql(MssqlValueRef<'r>),
}

impl Value for AnyValue {
    type Database = Any;

    fn as_ref(&self) -> <Self::Database as HasValueRef<'_>>::ValueRef {
        match &self.kind {
            #[cfg(feature = "postgres")]
            AnyValueKind::Postgres(value) => value.as_ref().into(),

            #[cfg(feature = "mysql")]
            AnyValueKind::MySql(value) => value.as_ref().into(),

            #[cfg(feature = "sqlite")]
            AnyValueKind::Sqlite(value) => value.as_ref().into(),

            #[cfg(feature = "mssql")]
            AnyValueKind::Mssql(value) => value.as_ref().into(),
        }
    }

    fn type_info(&self) -> Cow<'_, AnyTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        match &self.kind {
            #[cfg(feature = "postgres")]
            AnyValueKind::Postgres(value) => value.is_null(),

            #[cfg(feature = "mysql")]
            AnyValueKind::MySql(value) => value.is_null(),

            #[cfg(feature = "sqlite")]
            AnyValueKind::Sqlite(value) => value.is_null(),

            #[cfg(feature = "mssql")]
            AnyValueKind::Mssql(value) => value.is_null(),
        }
    }
}

impl<'r> ValueRef<'r> for AnyValueRef<'r> {
    type Database = Any;

    fn to_owned(&self) -> AnyValue {
        match &self.kind {
            #[cfg(feature = "postgres")]
            AnyValueRefKind::Postgres(value) => ValueRef::to_owned(value).into(),

            #[cfg(feature = "mysql")]
            AnyValueRefKind::MySql(value) => ValueRef::to_owned(value).into(),

            #[cfg(feature = "sqlite")]
            AnyValueRefKind::Sqlite(value) => ValueRef::to_owned(value).into(),

            #[cfg(feature = "mssql")]
            AnyValueRefKind::Mssql(value) => ValueRef::to_owned(value).into(),
        }
    }

    fn type_info(&self) -> Cow<'_, AnyTypeInfo> {
        Cow::Borrowed(&self.type_info)
    }

    fn is_null(&self) -> bool {
        match &self.kind {
            #[cfg(feature = "postgres")]
            AnyValueRefKind::Postgres(value) => value.is_null(),

            #[cfg(feature = "mysql")]
            AnyValueRefKind::MySql(value) => value.is_null(),

            #[cfg(feature = "sqlite")]
            AnyValueRefKind::Sqlite(value) => value.is_null(),

            #[cfg(feature = "mssql")]
            AnyValueRefKind::Mssql(value) => value.is_null(),
        }
    }
}
