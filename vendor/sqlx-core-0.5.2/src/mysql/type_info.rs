use std::fmt::{self, Display, Formatter};

use crate::mysql::protocol::text::{ColumnDefinition, ColumnFlags, ColumnType};
use crate::type_info::TypeInfo;

/// Type information for a MySql type.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct MySqlTypeInfo {
    pub(crate) r#type: ColumnType,
    pub(crate) flags: ColumnFlags,
    pub(crate) char_set: u16,

    // [max_size] for integer types, this is (M) in BIT(M) or TINYINT(M)
    #[cfg_attr(feature = "offline", serde(default))]
    pub(crate) max_size: Option<u32>,
}

impl MySqlTypeInfo {
    pub(crate) const fn binary(ty: ColumnType) -> Self {
        Self {
            r#type: ty,
            flags: ColumnFlags::BINARY,
            char_set: 63,
            max_size: None,
        }
    }

    #[doc(hidden)]
    pub const fn __enum() -> Self {
        Self {
            r#type: ColumnType::Enum,
            flags: ColumnFlags::BINARY,
            char_set: 63,
            max_size: None,
        }
    }

    #[doc(hidden)]
    pub fn __type_feature_gate(&self) -> Option<&'static str> {
        match self.r#type {
            ColumnType::Date | ColumnType::Time | ColumnType::Timestamp | ColumnType::Datetime => {
                Some("time")
            }

            ColumnType::Json => Some("json"),
            ColumnType::NewDecimal => Some("bigdecimal"),

            _ => None,
        }
    }

    pub(crate) fn from_column(column: &ColumnDefinition) -> Self {
        Self {
            r#type: column.r#type,
            flags: column.flags,
            char_set: column.char_set,
            max_size: Some(column.max_size),
        }
    }
}

impl Display for MySqlTypeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}

impl TypeInfo for MySqlTypeInfo {
    fn is_null(&self) -> bool {
        matches!(self.r#type, ColumnType::Null)
    }

    fn name(&self) -> &str {
        self.r#type.name(self.char_set, self.flags, self.max_size)
    }
}

impl PartialEq<MySqlTypeInfo> for MySqlTypeInfo {
    fn eq(&self, other: &MySqlTypeInfo) -> bool {
        if self.r#type != other.r#type {
            return false;
        }

        match self.r#type {
            ColumnType::Tiny
            | ColumnType::Short
            | ColumnType::Long
            | ColumnType::Int24
            | ColumnType::LongLong => {
                return self.flags.contains(ColumnFlags::UNSIGNED)
                    == other.flags.contains(ColumnFlags::UNSIGNED);
            }

            // for string types, check that our charset matches
            ColumnType::VarChar
            | ColumnType::Blob
            | ColumnType::TinyBlob
            | ColumnType::MediumBlob
            | ColumnType::LongBlob
            | ColumnType::String
            | ColumnType::VarString
            | ColumnType::Enum => {
                return self.char_set == other.char_set;
            }

            _ => {}
        }

        true
    }
}

impl Eq for MySqlTypeInfo {}

#[cfg(feature = "any")]
impl From<MySqlTypeInfo> for crate::any::AnyTypeInfo {
    #[inline]
    fn from(ty: MySqlTypeInfo) -> Self {
        crate::any::AnyTypeInfo(crate::any::type_info::AnyTypeInfoKind::MySql(ty))
    }
}
