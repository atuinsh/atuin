use crate::any::{Any, AnyColumn, AnyColumnIndex};
use crate::column::ColumnIndex;
use crate::database::HasValueRef;
use crate::error::Error;
use crate::row::Row;

#[cfg(feature = "postgres")]
use crate::postgres::PgRow;

#[cfg(feature = "mysql")]
use crate::mysql::MySqlRow;

#[cfg(feature = "sqlite")]
use crate::sqlite::SqliteRow;

#[cfg(feature = "mssql")]
use crate::mssql::MssqlRow;

pub struct AnyRow {
    pub(crate) kind: AnyRowKind,
    pub(crate) columns: Vec<AnyColumn>,
}

impl crate::row::private_row::Sealed for AnyRow {}

pub(crate) enum AnyRowKind {
    #[cfg(feature = "postgres")]
    Postgres(PgRow),

    #[cfg(feature = "mysql")]
    MySql(MySqlRow),

    #[cfg(feature = "sqlite")]
    Sqlite(SqliteRow),

    #[cfg(feature = "mssql")]
    Mssql(MssqlRow),
}

impl Row for AnyRow {
    type Database = Any;

    fn columns(&self) -> &[AnyColumn] {
        &self.columns
    }

    fn try_get_raw<I>(
        &self,
        index: I,
    ) -> Result<<Self::Database as HasValueRef<'_>>::ValueRef, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;

        match &self.kind {
            #[cfg(feature = "postgres")]
            AnyRowKind::Postgres(row) => row.try_get_raw(index).map(Into::into),

            #[cfg(feature = "mysql")]
            AnyRowKind::MySql(row) => row.try_get_raw(index).map(Into::into),

            #[cfg(feature = "sqlite")]
            AnyRowKind::Sqlite(row) => row.try_get_raw(index).map(Into::into),

            #[cfg(feature = "mssql")]
            AnyRowKind::Mssql(row) => row.try_get_raw(index).map(Into::into),
        }
    }
}

impl<'i> ColumnIndex<AnyRow> for &'i str
where
    &'i str: AnyColumnIndex,
{
    fn index(&self, row: &AnyRow) -> Result<usize, Error> {
        match &row.kind {
            #[cfg(feature = "postgres")]
            AnyRowKind::Postgres(row) => self.index(row),

            #[cfg(feature = "mysql")]
            AnyRowKind::MySql(row) => self.index(row),

            #[cfg(feature = "sqlite")]
            AnyRowKind::Sqlite(row) => self.index(row),

            #[cfg(feature = "mssql")]
            AnyRowKind::Mssql(row) => self.index(row),
        }
    }
}
