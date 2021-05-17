use crate::column::Column;
use crate::ext::ustr::UStr;
use crate::sqlite::{Sqlite, SqliteTypeInfo};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct SqliteColumn {
    pub(crate) name: UStr,
    pub(crate) ordinal: usize,
    pub(crate) type_info: SqliteTypeInfo,
}

impl crate::column::private_column::Sealed for SqliteColumn {}

impl Column for SqliteColumn {
    type Database = Sqlite;

    fn ordinal(&self) -> usize {
        self.ordinal
    }

    fn name(&self) -> &str {
        &*self.name
    }

    fn type_info(&self) -> &SqliteTypeInfo {
        &self.type_info
    }
}

#[cfg(feature = "any")]
impl From<SqliteColumn> for crate::any::AnyColumn {
    #[inline]
    fn from(column: SqliteColumn) -> Self {
        crate::any::AnyColumn {
            type_info: column.type_info.clone().into(),
            kind: crate::any::column::AnyColumnKind::Sqlite(column),
        }
    }
}
