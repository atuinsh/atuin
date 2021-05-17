use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::mssql::protocol::row::Row as ProtocolRow;
use crate::mssql::{Mssql, MssqlColumn, MssqlValueRef};
use crate::row::Row;
use crate::HashMap;
use std::sync::Arc;

pub struct MssqlRow {
    pub(crate) row: ProtocolRow,
    pub(crate) columns: Arc<Vec<MssqlColumn>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
}

impl crate::row::private_row::Sealed for MssqlRow {}

impl Row for MssqlRow {
    type Database = Mssql;

    fn columns(&self) -> &[MssqlColumn] {
        &*self.columns
    }

    fn try_get_raw<I>(&self, index: I) -> Result<MssqlValueRef<'_>, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;
        let value = MssqlValueRef {
            data: self.row.values[index].as_ref(),
            type_info: self.row.column_types[index].clone(),
        };

        Ok(value)
    }
}

impl ColumnIndex<MssqlRow> for &'_ str {
    fn index(&self, row: &MssqlRow) -> Result<usize, Error> {
        row.column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .map(|v| *v)
    }
}

#[cfg(feature = "any")]
impl From<MssqlRow> for crate::any::AnyRow {
    #[inline]
    fn from(row: MssqlRow) -> Self {
        crate::any::AnyRow {
            columns: row.columns.iter().map(|col| col.clone().into()).collect(),
            kind: crate::any::row::AnyRowKind::Mssql(row),
        }
    }
}
