use bytes::{Buf, Bytes};

use crate::error::Error;

#[derive(Debug)]
pub(crate) struct Order {
    columns: Bytes,
}

impl Order {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let len = buf.get_u16_le();
        let columns = buf.split_to(len as usize);

        Ok(Self { columns })
    }
}
