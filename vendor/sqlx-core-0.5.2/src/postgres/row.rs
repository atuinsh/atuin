use crate::column::ColumnIndex;
use crate::error::Error;
use crate::postgres::message::DataRow;
use crate::postgres::statement::PgStatementMetadata;
use crate::postgres::value::PgValueFormat;
use crate::postgres::{PgColumn, PgValueRef, Postgres};
use crate::row::Row;
use std::sync::Arc;

/// Implementation of [`Row`] for PostgreSQL.
pub struct PgRow {
    pub(crate) data: DataRow,
    pub(crate) format: PgValueFormat,
    pub(crate) metadata: Arc<PgStatementMetadata>,
}

impl crate::row::private_row::Sealed for PgRow {}

impl Row for PgRow {
    type Database = Postgres;

    fn columns(&self) -> &[PgColumn] {
        &self.metadata.columns
    }

    fn try_get_raw<I>(&self, index: I) -> Result<PgValueRef<'_>, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;
        let column = &self.metadata.columns[index];
        let value = self.data.get(index);

        Ok(PgValueRef {
            format: self.format,
            row: Some(&self.data.storage),
            type_info: column.type_info.clone(),
            value,
        })
    }
}

impl ColumnIndex<PgRow> for &'_ str {
    fn index(&self, row: &PgRow) -> Result<usize, Error> {
        row.metadata
            .column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .map(|v| *v)
    }
}

#[cfg(feature = "any")]
impl From<PgRow> for crate::any::AnyRow {
    #[inline]
    fn from(row: PgRow) -> Self {
        crate::any::AnyRow {
            columns: row
                .metadata
                .columns
                .iter()
                .map(|col| col.clone().into())
                .collect(),

            kind: crate::any::row::AnyRowKind::Postgres(row),
        }
    }
}
