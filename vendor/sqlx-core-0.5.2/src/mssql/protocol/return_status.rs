use bytes::{Buf, Bytes};

use crate::error::Error;

#[derive(Debug)]
pub(crate) struct ReturnStatus {
    value: i32,
}

impl ReturnStatus {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let value = buf.get_i32_le();

        Ok(Self { value })
    }
}
