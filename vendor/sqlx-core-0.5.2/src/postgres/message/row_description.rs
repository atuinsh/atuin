use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::io::{BufExt, Decode};

#[derive(Debug)]
pub struct RowDescription {
    pub fields: Vec<Field>,
}

#[derive(Debug)]
pub struct Field {
    /// The name of the field.
    pub name: String,

    /// If the field can be identified as a column of a specific table, the
    /// object ID of the table; otherwise zero.
    pub relation_id: Option<i32>,

    /// If the field can be identified as a column of a specific table, the attribute number of
    /// the column; otherwise zero.
    pub relation_attribute_no: Option<i16>,

    /// The object ID of the field's data type.
    pub data_type_id: u32,

    /// The data type size (see pg_type.typlen). Note that negative values denote
    /// variable-width types.
    pub data_type_size: i16,

    /// The type modifier (see pg_attribute.atttypmod). The meaning of the
    /// modifier is type-specific.
    pub type_modifier: i32,

    /// The format code being used for the field.
    pub format: i16,
}

impl Decode<'_> for RowDescription {
    fn decode_with(mut buf: Bytes, _: ()) -> Result<Self, Error> {
        let cnt = buf.get_u16();
        let mut fields = Vec::with_capacity(cnt as usize);

        for _ in 0..cnt {
            let name = buf.get_str_nul()?.to_owned();
            let relation_id = buf.get_i32();
            let relation_attribute_no = buf.get_i16();
            let data_type_id = buf.get_u32();
            let data_type_size = buf.get_i16();
            let type_modifier = buf.get_i32();
            let format = buf.get_i16();

            fields.push(Field {
                name,
                relation_id: if relation_id == 0 {
                    None
                } else {
                    Some(relation_id)
                },
                relation_attribute_no: if relation_attribute_no == 0 {
                    None
                } else {
                    Some(relation_attribute_no)
                },
                data_type_id,
                data_type_size,
                type_modifier,
                format,
            })
        }

        Ok(Self { fields })
    }
}

// TODO: Unit Test RowDescription
// TODO: Benchmark RowDescription
