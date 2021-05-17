use bitflags::bitflags;
use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::mssql::io::MssqlBufExt;
use crate::mssql::protocol::type_info::TypeInfo;
use crate::mssql::MssqlColumn;
use crate::HashMap;

#[derive(Debug)]
pub(crate) struct ColMetaData;

#[derive(Debug)]
pub(crate) struct ColumnData {
    // The user type ID of the data type of the column. Depending on the TDS version that is used,
    // valid values are 0x0000 or 0x00000000, with the exceptions of data type
    // TIMESTAMP (0x0050 or 0x00000050) and alias types (greater than 0x00FF or 0x000000FF).
    pub(crate) user_type: u32,

    pub(crate) flags: Flags,
    pub(crate) type_info: TypeInfo,

    // TODO: pub(crate) table_name: Option<Vec<String>>,
    // TODO: crypto_meta_data: Option<CryptoMetaData>,

    // The column name. It contains the column name length and column name.
    pub(crate) col_name: String,
}

bitflags! {
    #[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
    pub struct Flags: u16 {
        // Its value is 1 if the column is nullable.
        const NULLABLE = 0x0001;

        // Set to 1 for string columns with binary collation and always for the XML data type.
        // Set to 0 otherwise.
        const CASE_SEN = 0x0002;

        // usUpdateable is a 2-bit field. Its value is 0 if column is read-only, 1 if column is
        // read/write and2 if updateable is unknown.
        const UPDATEABLE1 = 0x0004;
        const UPDATEABLE2 = 0x0008;

        // Its value is 1 if the column is an identity column.
        const IDENITTY = 0x0010;

        // Its value is 1 if the column is a COMPUTED column.
        const COMPUTED = 0x0020;

        // Its value is 1 if the column is a fixed-length common language runtime
        // user-defined type (CLR UDT).
        const FIXED_LEN_CLR_TYPE = 0x0100;

        // fSparseColumnSet, introduced in TDSversion 7.3.B, is a bit flag. Its value is 1 if the
        // column is the special XML column for the sparse column set. For information about using
        // column sets, see [MSDN-ColSets]
        const SPARSE_COLUMN_SET = 0x0200;

        // Its value is 1 if the column is encrypted transparently and
        // has to be decrypted to view the plaintext value. This flag is valid when the column
        // encryption feature is negotiated between client and server and is turned on.
        const ENCRYPTED = 0x0400;

        // Its value is 1 if the column is part of a hidden primary key created to support a
        // T-SQL SELECT statement containing FOR BROWSE.
        const HIDDEN = 0x0800;

        // Its value is 1 if the column is part of a primary key for the row
        // and the T-SQL SELECT statement contains FOR BROWSE.
        const KEY = 0x1000;

        // Its value is 1 if it is unknown whether the column might be nullable.
        const NULLABLE_UNKNOWN = 0x2000;
    }
}

impl ColMetaData {
    pub(crate) fn get(
        buf: &mut Bytes,
        columns: &mut Vec<MssqlColumn>,
        column_names: &mut HashMap<UStr, usize>,
    ) -> Result<(), Error> {
        columns.clear();
        column_names.clear();

        let mut count = buf.get_u16_le();
        let mut ordinal = 0;

        if count == 0xffff {
            // In the event that the client requested no metadata to be returned, the value of
            // Count will be 0xFFFF. This has the same effect on Count as a
            // zero value (for example, no ColumnData is sent).
            count = 0;
        } else {
            columns.reserve(count as usize);
        }

        while count > 0 {
            let col = MssqlColumn::new(ColumnData::get(buf)?, ordinal);

            column_names.insert(col.name.clone(), ordinal);
            columns.push(col);

            count -= 1;
            ordinal += 1;
        }

        Ok(())
    }
}

impl ColumnData {
    fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let user_type = buf.get_u32_le();
        let flags = Flags::from_bits_truncate(buf.get_u16_le());
        let type_info = TypeInfo::get(buf)?;

        // TODO: table_name
        // TODO: crypto_meta_data

        let name = buf.get_b_varchar()?;

        Ok(Self {
            user_type,
            flags,
            type_info,
            col_name: name,
        })
    }
}
