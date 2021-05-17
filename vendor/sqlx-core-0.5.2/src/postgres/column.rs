use crate::column::Column;
use crate::ext::ustr::UStr;
use crate::postgres::{PgTypeInfo, Postgres};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct PgColumn {
    pub(crate) ordinal: usize,
    pub(crate) name: UStr,
    pub(crate) type_info: PgTypeInfo,
    #[cfg_attr(feature = "offline", serde(skip))]
    pub(crate) relation_id: Option<i32>,
    #[cfg_attr(feature = "offline", serde(skip))]
    pub(crate) relation_attribute_no: Option<i16>,
}

impl crate::column::private_column::Sealed for PgColumn {}

impl Column for PgColumn {
    type Database = Postgres;

    fn ordinal(&self) -> usize {
        self.ordinal
    }

    fn name(&self) -> &str {
        &*self.name
    }

    fn type_info(&self) -> &PgTypeInfo {
        &self.type_info
    }
}

#[cfg(feature = "any")]
impl From<PgColumn> for crate::any::AnyColumn {
    #[inline]
    fn from(column: PgColumn) -> Self {
        crate::any::AnyColumn {
            type_info: column.type_info.clone().into(),
            kind: crate::any::column::AnyColumnKind::Postgres(column),
        }
    }
}
