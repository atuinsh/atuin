use bytes::Bytes;

use crate::error::Error;
use crate::io::BufExt;
use crate::mssql::{MssqlColumn, MssqlTypeInfo};

#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) column_types: Vec<MssqlTypeInfo>,
    pub(crate) values: Vec<Option<Bytes>>,
}

impl Row {
    pub(crate) fn get(
        buf: &mut Bytes,
        nullable: bool,
        columns: &[MssqlColumn],
    ) -> Result<Self, Error> {
        let mut values = Vec::with_capacity(columns.len());
        let mut column_types = Vec::with_capacity(columns.len());

        let nulls = if nullable {
            buf.get_bytes((columns.len() + 7) / 8)
        } else {
            Bytes::from_static(b"")
        };

        for (i, column) in columns.iter().enumerate() {
            column_types.push(column.type_info.clone());

            if !(column.type_info.0.is_null() || (nullable && (nulls[i / 8] & (1 << (i % 8))) != 0))
            {
                values.push(column.type_info.0.get_value(buf));
            } else {
                values.push(None);
            }
        }

        Ok(Self {
            values,
            column_types,
        })
    }
}
