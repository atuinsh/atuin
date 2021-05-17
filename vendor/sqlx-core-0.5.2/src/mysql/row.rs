use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::mysql::{protocol, MySql, MySqlColumn, MySqlValueFormat, MySqlValueRef};
use crate::row::Row;
use crate::HashMap;
use std::sync::Arc;

/// Implementation of [`Row`] for MySQL.
#[derive(Debug)]
pub struct MySqlRow {
    pub(crate) row: protocol::Row,
    pub(crate) format: MySqlValueFormat,
    pub(crate) columns: Arc<Vec<MySqlColumn>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
}

impl crate::row::private_row::Sealed for MySqlRow {}

impl Row for MySqlRow {
    type Database = MySql;

    fn columns(&self) -> &[MySqlColumn] {
        &self.columns
    }

    fn try_get_raw<I>(&self, index: I) -> Result<MySqlValueRef<'_>, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;
        let column = &self.columns[index];
        let value = self.row.get(index);

        Ok(MySqlValueRef {
            format: self.format,
            row: Some(&self.row.storage),
            type_info: column.type_info.clone(),
            value,
        })
    }
}

impl ColumnIndex<MySqlRow> for &'_ str {
    fn index(&self, row: &MySqlRow) -> Result<usize, Error> {
        row.column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .map(|v| *v)
    }
}

#[cfg(feature = "any")]
impl From<MySqlRow> for crate::any::AnyRow {
    #[inline]
    fn from(row: MySqlRow) -> Self {
        crate::any::AnyRow {
            columns: row.columns.iter().map(|col| col.clone().into()).collect(),

            kind: crate::any::row::AnyRowKind::MySql(row),
        }
    }
}
