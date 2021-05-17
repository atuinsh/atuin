use crate::{
    decode::Decode,
    encode::{Encode, IsNull},
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef, Postgres},
    types::Type,
};
use bit_vec::BitVec;
use bytes::Buf;
use std::{io, mem};

impl Type<Postgres> for BitVec {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::VARBIT
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        *ty == PgTypeInfo::BIT || *ty == PgTypeInfo::VARBIT
    }
}

impl Type<Postgres> for [BitVec] {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::VARBIT_ARRAY
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        *ty == PgTypeInfo::BIT_ARRAY || *ty == PgTypeInfo::VARBIT_ARRAY
    }
}

impl Type<Postgres> for Vec<BitVec> {
    fn type_info() -> PgTypeInfo {
        <[BitVec] as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <[BitVec] as Type<Postgres>>::compatible(ty)
    }
}

impl Encode<'_, Postgres> for BitVec {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&(self.len() as i32).to_be_bytes());
        buf.extend(self.to_bytes());

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        mem::size_of::<i32>() + self.len()
    }
}

impl Decode<'_, Postgres> for BitVec {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        match value.format() {
            PgValueFormat::Binary => {
                let mut bytes = value.as_bytes()?;
                let len = bytes.get_i32();

                if len < 0 {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Negative VARBIT length.",
                    ))?
                }

                // The smallest amount of data we can read is one byte
                let bytes_len = (len as usize + 7) / 8;

                if bytes.remaining() != bytes_len {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VARBIT length mismatch.",
                    ))?;
                }

                let mut bitvec = BitVec::from_bytes(&bytes);

                // Chop off zeroes from the back. We get bits in bytes, so if
                // our bitvec is not in full bytes, extra zeroes are added to
                // the end.
                while bitvec.len() > len as usize {
                    bitvec.pop();
                }

                Ok(bitvec)
            }
            PgValueFormat::Text => {
                let s = value.as_str()?;
                let mut bit_vec = BitVec::with_capacity(s.len());

                for c in s.chars() {
                    match c {
                        '0' => bit_vec.push(false),
                        '1' => bit_vec.push(true),
                        _ => {
                            Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "VARBIT data contains other characters than 1 or 0.",
                            ))?;
                        }
                    }
                }

                Ok(bit_vec)
            }
        }
    }
}
