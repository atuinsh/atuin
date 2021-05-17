use std::fmt::{self, Display, Formatter};

use crate::type_info::TypeInfo;

#[cfg(feature = "postgres")]
use crate::postgres::PgTypeInfo;

#[cfg(feature = "mysql")]
use crate::mysql::MySqlTypeInfo;

#[cfg(feature = "sqlite")]
use crate::sqlite::SqliteTypeInfo;

#[cfg(feature = "mssql")]
use crate::mssql::MssqlTypeInfo;

#[derive(Debug, Clone, PartialEq)]
pub struct AnyTypeInfo(pub(crate) AnyTypeInfoKind);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AnyTypeInfoKind {
    #[cfg(feature = "postgres")]
    Postgres(PgTypeInfo),

    #[cfg(feature = "mysql")]
    MySql(MySqlTypeInfo),

    #[cfg(feature = "sqlite")]
    Sqlite(SqliteTypeInfo),

    #[cfg(feature = "mssql")]
    Mssql(MssqlTypeInfo),
}

impl TypeInfo for AnyTypeInfo {
    fn is_null(&self) -> bool {
        match &self.0 {
            #[cfg(feature = "postgres")]
            AnyTypeInfoKind::Postgres(ty) => ty.is_null(),

            #[cfg(feature = "mysql")]
            AnyTypeInfoKind::MySql(ty) => ty.is_null(),

            #[cfg(feature = "sqlite")]
            AnyTypeInfoKind::Sqlite(ty) => ty.is_null(),

            #[cfg(feature = "mssql")]
            AnyTypeInfoKind::Mssql(ty) => ty.is_null(),
        }
    }

    fn name(&self) -> &str {
        match &self.0 {
            #[cfg(feature = "postgres")]
            AnyTypeInfoKind::Postgres(ty) => ty.name(),

            #[cfg(feature = "mysql")]
            AnyTypeInfoKind::MySql(ty) => ty.name(),

            #[cfg(feature = "sqlite")]
            AnyTypeInfoKind::Sqlite(ty) => ty.name(),

            #[cfg(feature = "mssql")]
            AnyTypeInfoKind::Mssql(ty) => ty.name(),
        }
    }
}

impl Display for AnyTypeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            #[cfg(feature = "postgres")]
            AnyTypeInfoKind::Postgres(ty) => ty.fmt(f),

            #[cfg(feature = "mysql")]
            AnyTypeInfoKind::MySql(ty) => ty.fmt(f),

            #[cfg(feature = "sqlite")]
            AnyTypeInfoKind::Sqlite(ty) => ty.fmt(f),

            #[cfg(feature = "mssql")]
            AnyTypeInfoKind::Mssql(ty) => ty.fmt(f),
        }
    }
}
