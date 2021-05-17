use crate::column::Column;
use crate::ext::ustr::UStr;
use crate::mysql::protocol::text::ColumnFlags;
use crate::mysql::{MySql, MySqlTypeInfo};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct MySqlColumn {
    pub(crate) ordinal: usize,
    pub(crate) name: UStr,
    pub(crate) type_info: MySqlTypeInfo,

    #[cfg_attr(feature = "offline", serde(skip))]
    pub(crate) flags: Option<ColumnFlags>,
}

impl crate::column::private_column::Sealed for MySqlColumn {}

impl Column for MySqlColumn {
    type Database = MySql;

    fn ordinal(&self) -> usize {
        self.ordinal
    }

    fn name(&self) -> &str {
        &*self.name
    }

    fn type_info(&self) -> &MySqlTypeInfo {
        &self.type_info
    }
}

#[cfg(feature = "any")]
impl From<MySqlColumn> for crate::any::AnyColumn {
    #[inline]
    fn from(column: MySqlColumn) -> Self {
        crate::any::AnyColumn {
            type_info: column.type_info.clone().into(),
            kind: crate::any::column::AnyColumnKind::MySql(column),
        }
    }
}
