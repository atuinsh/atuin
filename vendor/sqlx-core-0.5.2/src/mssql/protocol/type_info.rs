use bitflags::bitflags;
use bytes::{Buf, Bytes};
use encoding_rs::Encoding;

use crate::encode::{Encode, IsNull};
use crate::error::Error;
use crate::mssql::Mssql;

bitflags! {
    #[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
    pub(crate) struct CollationFlags: u8 {
        const IGNORE_CASE = (1 << 0);
        const IGNORE_ACCENT = (1 << 1);
        const IGNORE_WIDTH = (1 << 2);
        const IGNORE_KANA = (1 << 3);
        const BINARY = (1 << 4);
        const BINARY2 = (1 << 5);
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct Collation {
    pub(crate) locale: u32,
    pub(crate) flags: CollationFlags,
    pub(crate) sort: u8,
    pub(crate) version: u8,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub(crate) enum DataType {
    // fixed-length data types
    // https://docs.microsoft.com/en-us/openspecs/sql_server_protocols/ms-sstds/d33ef17b-7e53-4380-ad11-2ba42c8dda8d
    Null = 0x1f,
    TinyInt = 0x30,
    Bit = 0x32,
    SmallInt = 0x34,
    Int = 0x38,
    SmallDateTime = 0x3a,
    Real = 0x3b,
    Money = 0x3c,
    DateTime = 0x3d,
    Float = 0x3e,
    SmallMoney = 0x7a,
    BigInt = 0x7f,

    // variable-length data types
    // https://docs.microsoft.com/en-us/openspecs/windows_protocols/ms-tds/ce3183a6-9d89-47e8-a02f-de5a1a1303de

    // byte length
    Guid = 0x24,
    IntN = 0x26,
    Decimal = 0x37, // legacy
    Numeric = 0x3f, // legacy
    BitN = 0x68,
    DecimalN = 0x6a,
    NumericN = 0x6c,
    FloatN = 0x6d,
    MoneyN = 0x6e,
    DateTimeN = 0x6f,
    DateN = 0x28,
    TimeN = 0x29,
    DateTime2N = 0x2a,
    DateTimeOffsetN = 0x2b,
    Char = 0x2f,      // legacy
    VarChar = 0x27,   // legacy
    Binary = 0x2d,    // legacy
    VarBinary = 0x25, // legacy

    // short length
    BigVarBinary = 0xa5,
    BigVarChar = 0xa7,
    BigBinary = 0xad,
    BigChar = 0xaf,
    NVarChar = 0xe7,
    NChar = 0xef,
    Xml = 0xf1,
    UserDefined = 0xf0,

    // long length
    Text = 0x23,
    Image = 0x22,
    NText = 0x63,
    Variant = 0x62,
}

// http://msdn.microsoft.com/en-us/library/dd358284.aspx
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct TypeInfo {
    pub(crate) ty: DataType,
    pub(crate) size: u32,
    pub(crate) scale: u8,
    pub(crate) precision: u8,
    pub(crate) collation: Option<Collation>,
}

impl TypeInfo {
    pub(crate) const fn new(ty: DataType, size: u32) -> Self {
        Self {
            ty,
            size,
            scale: 0,
            precision: 0,
            collation: None,
        }
    }

    pub(crate) fn encoding(&self) -> Result<&'static Encoding, Error> {
        match self.ty {
            DataType::NChar | DataType::NVarChar => Ok(encoding_rs::UTF_16LE),

            DataType::VarChar | DataType::Char | DataType::BigChar | DataType::BigVarChar => {
                // unwrap: impossible to unwrap here, collation will be set
                Ok(match self.collation.unwrap().locale {
                    // This is the Western encoding for Windows. It is an extension of ISO-8859-1,
                    // which is known as Latin 1.
                    0x0409 => encoding_rs::WINDOWS_1252,

                    locale => {
                        return Err(err_protocol!("unsupported locale 0x{:?}", locale));
                    }
                })
            }

            _ => {
                // default to UTF-8 for anything
                // else coming in here
                Ok(encoding_rs::UTF_8)
            }
        }
    }

    // reads a TYPE_INFO from the buffer
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let ty = DataType::get(buf)?;

        Ok(match ty {
            DataType::Null => Self::new(ty, 0),

            DataType::TinyInt | DataType::Bit => Self::new(ty, 1),

            DataType::SmallInt => Self::new(ty, 2),

            DataType::Int | DataType::SmallDateTime | DataType::Real | DataType::SmallMoney => {
                Self::new(ty, 4)
            }

            DataType::BigInt | DataType::Money | DataType::DateTime | DataType::Float => {
                Self::new(ty, 8)
            }

            DataType::DateN => Self::new(ty, 3),

            DataType::TimeN | DataType::DateTime2N | DataType::DateTimeOffsetN => {
                let scale = buf.get_u8();

                let mut size = match scale {
                    0 | 1 | 2 => 3,
                    3 | 4 => 4,
                    5 | 6 | 7 => 5,

                    scale => {
                        return Err(err_protocol!("invalid scale {} for type {:?}", scale, ty));
                    }
                };

                match ty {
                    DataType::DateTime2N => {
                        size += 3;
                    }

                    DataType::DateTimeOffsetN => {
                        size += 5;
                    }

                    _ => {}
                }

                Self {
                    scale,
                    size,
                    ty,
                    precision: 0,
                    collation: None,
                }
            }

            DataType::Guid
            | DataType::IntN
            | DataType::BitN
            | DataType::FloatN
            | DataType::MoneyN
            | DataType::DateTimeN
            | DataType::Char
            | DataType::VarChar
            | DataType::Binary
            | DataType::VarBinary => Self::new(ty, buf.get_u8() as u32),

            DataType::Decimal | DataType::Numeric | DataType::DecimalN | DataType::NumericN => {
                let size = buf.get_u8() as u32;
                let precision = buf.get_u8();
                let scale = buf.get_u8();

                Self {
                    size,
                    precision,
                    scale,
                    ty,
                    collation: None,
                }
            }

            DataType::BigVarBinary | DataType::BigBinary => Self::new(ty, buf.get_u16_le() as u32),

            DataType::BigVarChar | DataType::BigChar | DataType::NVarChar | DataType::NChar => {
                let size = buf.get_u16_le() as u32;
                let collation = Collation::get(buf);

                Self {
                    ty,
                    size,
                    collation: Some(collation),
                    scale: 0,
                    precision: 0,
                }
            }

            _ => {
                return Err(err_protocol!("unsupported data type {:?}", ty));
            }
        })
    }

    // writes a TYPE_INFO to the buffer
    pub(crate) fn put(&self, buf: &mut Vec<u8>) {
        buf.push(self.ty as u8);

        match self.ty {
            DataType::Null
            | DataType::TinyInt
            | DataType::Bit
            | DataType::SmallInt
            | DataType::Int
            | DataType::SmallDateTime
            | DataType::Real
            | DataType::SmallMoney
            | DataType::BigInt
            | DataType::Money
            | DataType::DateTime
            | DataType::Float
            | DataType::DateN => {
                // nothing to do
            }

            DataType::TimeN | DataType::DateTime2N | DataType::DateTimeOffsetN => {
                buf.push(self.scale);
            }

            DataType::Guid
            | DataType::IntN
            | DataType::BitN
            | DataType::FloatN
            | DataType::MoneyN
            | DataType::DateTimeN
            | DataType::Char
            | DataType::VarChar
            | DataType::Binary
            | DataType::VarBinary => {
                buf.push(self.size as u8);
            }

            DataType::Decimal | DataType::Numeric | DataType::DecimalN | DataType::NumericN => {
                buf.push(self.size as u8);
                buf.push(self.precision);
                buf.push(self.scale);
            }

            DataType::BigVarBinary | DataType::BigBinary => {
                buf.extend(&(self.size as u16).to_le_bytes());
            }

            DataType::BigVarChar | DataType::BigChar | DataType::NVarChar | DataType::NChar => {
                buf.extend(&(self.size as u16).to_le_bytes());

                if let Some(collation) = &self.collation {
                    collation.put(buf);
                } else {
                    buf.extend(&0_u32.to_le_bytes());
                    buf.push(0);
                }
            }

            _ => {
                unimplemented!("unsupported data type {:?}", self.ty);
            }
        }
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self.ty, DataType::Null)
    }

    pub(crate) fn get_value(&self, buf: &mut Bytes) -> Option<Bytes> {
        match self.ty {
            DataType::Null
            | DataType::TinyInt
            | DataType::Bit
            | DataType::SmallInt
            | DataType::Int
            | DataType::SmallDateTime
            | DataType::Real
            | DataType::Money
            | DataType::DateTime
            | DataType::Float
            | DataType::SmallMoney
            | DataType::BigInt => Some(buf.split_to(self.size as usize)),

            DataType::Guid
            | DataType::IntN
            | DataType::Decimal
            | DataType::Numeric
            | DataType::BitN
            | DataType::DecimalN
            | DataType::NumericN
            | DataType::FloatN
            | DataType::MoneyN
            | DataType::DateTimeN
            | DataType::DateN
            | DataType::TimeN
            | DataType::DateTime2N
            | DataType::DateTimeOffsetN => {
                let size = buf.get_u8();

                if size == 0 || size == 0xFF {
                    None
                } else {
                    Some(buf.split_to(size as usize))
                }
            }

            DataType::Char | DataType::VarChar | DataType::Binary | DataType::VarBinary => {
                let size = buf.get_u8();

                if size == 0xFF {
                    None
                } else {
                    Some(buf.split_to(size as usize))
                }
            }

            DataType::BigVarBinary
            | DataType::BigVarChar
            | DataType::BigBinary
            | DataType::BigChar
            | DataType::NVarChar
            | DataType::NChar
            | DataType::Xml
            | DataType::UserDefined => {
                let size = buf.get_u16_le();

                if size == 0xFF_FF {
                    None
                } else {
                    Some(buf.split_to(size as usize))
                }
            }

            DataType::Text | DataType::Image | DataType::NText | DataType::Variant => {
                let size = buf.get_u32_le();

                if size == 0xFFFF_FFFF {
                    None
                } else {
                    Some(buf.split_to(size as usize))
                }
            }
        }
    }

    pub(crate) fn put_value<'q, T: Encode<'q, Mssql>>(&self, buf: &mut Vec<u8>, value: T) {
        match self.ty {
            DataType::Null
            | DataType::TinyInt
            | DataType::Bit
            | DataType::SmallInt
            | DataType::Int
            | DataType::SmallDateTime
            | DataType::Real
            | DataType::Money
            | DataType::DateTime
            | DataType::Float
            | DataType::SmallMoney
            | DataType::BigInt => {
                self.put_fixed_value(buf, value);
            }

            DataType::Guid
            | DataType::IntN
            | DataType::Decimal
            | DataType::Numeric
            | DataType::BitN
            | DataType::DecimalN
            | DataType::NumericN
            | DataType::FloatN
            | DataType::MoneyN
            | DataType::DateTimeN
            | DataType::DateN
            | DataType::TimeN
            | DataType::DateTime2N
            | DataType::DateTimeOffsetN
            | DataType::Char
            | DataType::VarChar
            | DataType::Binary
            | DataType::VarBinary => {
                self.put_byte_len_value(buf, value);
            }

            DataType::BigVarBinary
            | DataType::BigVarChar
            | DataType::BigBinary
            | DataType::BigChar
            | DataType::NVarChar
            | DataType::NChar
            | DataType::Xml
            | DataType::UserDefined => {
                self.put_short_len_value(buf, value);
            }

            DataType::Text | DataType::Image | DataType::NText | DataType::Variant => {
                self.put_long_len_value(buf, value);
            }
        }
    }

    pub(crate) fn put_fixed_value<'q, T: Encode<'q, Mssql>>(&self, buf: &mut Vec<u8>, value: T) {
        let _ = value.encode(buf);
    }

    pub(crate) fn put_byte_len_value<'q, T: Encode<'q, Mssql>>(&self, buf: &mut Vec<u8>, value: T) {
        let offset = buf.len();
        buf.push(0);

        let size = if let IsNull::Yes = value.encode(buf) {
            0xFF
        } else {
            (buf.len() - offset - 1) as u8
        };

        buf[offset] = size;
    }

    pub(crate) fn put_short_len_value<'q, T: Encode<'q, Mssql>>(
        &self,
        buf: &mut Vec<u8>,
        value: T,
    ) {
        let offset = buf.len();
        buf.extend(&0_u16.to_le_bytes());

        let size = if let IsNull::Yes = value.encode(buf) {
            0xFFFF
        } else {
            (buf.len() - offset - 2) as u16
        };

        buf[offset..(offset + 2)].copy_from_slice(&size.to_le_bytes());
    }

    pub(crate) fn put_long_len_value<'q, T: Encode<'q, Mssql>>(&self, buf: &mut Vec<u8>, value: T) {
        let offset = buf.len();
        buf.extend(&0_u32.to_le_bytes());

        let size = if let IsNull::Yes = value.encode(buf) {
            0xFFFF_FFFF
        } else {
            (buf.len() - offset - 4) as u32
        };

        buf[offset..(offset + 4)].copy_from_slice(&size.to_le_bytes());
    }

    pub(crate) fn name(&self) -> &'static str {
        match self.ty {
            DataType::Null => "NULL",
            DataType::TinyInt => "TINYINT",
            DataType::SmallInt => "SMALLINT",
            DataType::Int => "INT",
            DataType::BigInt => "BIGINT",
            DataType::Real => "REAL",
            DataType::Float => "FLOAT",

            DataType::IntN => match self.size {
                1 => "TINYINT",
                2 => "SMALLINT",
                4 => "INT",
                8 => "BIGINT",

                _ => unreachable!("invalid size {} for int"),
            },

            DataType::FloatN => match self.size {
                4 => "REAL",
                8 => "FLOAT",

                _ => unreachable!("invalid size {} for float"),
            },

            DataType::VarChar => "VARCHAR",
            DataType::NVarChar => "NVARCHAR",
            DataType::BigVarChar => "BIGVARCHAR",
            DataType::Char => "CHAR",
            DataType::BigChar => "BIGCHAR",
            DataType::NChar => "NCHAR",

            _ => unimplemented!("name: unsupported data type {:?}", self.ty),
        }
    }

    pub(crate) fn fmt(&self, s: &mut String) {
        match self.ty {
            DataType::Null => s.push_str("nvarchar(1)"),
            DataType::TinyInt => s.push_str("tinyint"),
            DataType::SmallInt => s.push_str("smallint"),
            DataType::Int => s.push_str("int"),
            DataType::BigInt => s.push_str("bigint"),
            DataType::Real => s.push_str("real"),
            DataType::Float => s.push_str("float"),
            DataType::Bit => s.push_str("bit"),

            DataType::IntN => s.push_str(match self.size {
                1 => "tinyint",
                2 => "smallint",
                4 => "int",
                8 => "bigint",

                _ => unreachable!("invalid size {} for int"),
            }),

            DataType::FloatN => s.push_str(match self.size {
                4 => "real",
                8 => "float",

                _ => unreachable!("invalid size {} for float"),
            }),

            DataType::VarChar
            | DataType::NVarChar
            | DataType::BigVarChar
            | DataType::Char
            | DataType::BigChar
            | DataType::NChar => {
                // name
                s.push_str(match self.ty {
                    DataType::VarChar => "varchar",
                    DataType::NVarChar => "nvarchar",
                    DataType::BigVarChar => "bigvarchar",
                    DataType::Char => "char",
                    DataType::BigChar => "bigchar",
                    DataType::NChar => "nchar",

                    _ => unreachable!(),
                });

                // size
                if self.size < 8000 && self.size > 0 {
                    s.push_str("(");
                    let _ = itoa::fmt(&mut *s, self.size);
                    s.push_str(")");
                } else {
                    s.push_str("(max)");
                }
            }

            DataType::BitN => {
                s.push_str("bit");
            }

            _ => unimplemented!("fmt: unsupported data type {:?}", self.ty),
        }
    }
}

impl DataType {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        Ok(match buf.get_u8() {
            0x1f => DataType::Null,
            0x30 => DataType::TinyInt,
            0x32 => DataType::Bit,
            0x34 => DataType::SmallInt,
            0x38 => DataType::Int,
            0x3a => DataType::SmallDateTime,
            0x3b => DataType::Real,
            0x3c => DataType::Money,
            0x3d => DataType::DateTime,
            0x3e => DataType::Float,
            0x7a => DataType::SmallMoney,
            0x7f => DataType::BigInt,
            0x24 => DataType::Guid,
            0x26 => DataType::IntN,
            0x37 => DataType::Decimal,
            0x3f => DataType::Numeric,
            0x68 => DataType::BitN,
            0x6a => DataType::DecimalN,
            0x6c => DataType::NumericN,
            0x6d => DataType::FloatN,
            0x6e => DataType::MoneyN,
            0x6f => DataType::DateTimeN,
            0x28 => DataType::DateN,
            0x29 => DataType::TimeN,
            0x2a => DataType::DateTime2N,
            0x2b => DataType::DateTimeOffsetN,
            0x2f => DataType::Char,
            0x27 => DataType::VarChar,
            0x2d => DataType::Binary,
            0x25 => DataType::VarBinary,
            0xa5 => DataType::BigVarBinary,
            0xa7 => DataType::BigVarChar,
            0xad => DataType::BigBinary,
            0xaf => DataType::BigChar,
            0xe7 => DataType::NVarChar,
            0xef => DataType::NChar,
            0xf1 => DataType::Xml,
            0xf0 => DataType::UserDefined,
            0x23 => DataType::Text,
            0x22 => DataType::Image,
            0x63 => DataType::NText,
            0x62 => DataType::Variant,

            ty => {
                return Err(err_protocol!("unknown data type 0x{:02x}", ty));
            }
        })
    }
}

impl Collation {
    pub(crate) fn get(buf: &mut Bytes) -> Collation {
        let locale_sort_version = buf.get_u32_le();
        let locale = locale_sort_version & 0xfffff;
        let flags = CollationFlags::from_bits_truncate(((locale_sort_version >> 20) & 0xFF) as u8);
        let version = (locale_sort_version >> 28) as u8;
        let sort = buf.get_u8();

        Collation {
            locale,
            flags,
            sort,
            version,
        }
    }

    pub(crate) fn put(&self, buf: &mut Vec<u8>) {
        let locale_sort_version =
            self.locale | ((self.flags.bits() as u32) << 20) | ((self.version as u32) << 28);

        buf.extend(&locale_sort_version.to_le_bytes());
        buf.push(self.sort);
    }
}
