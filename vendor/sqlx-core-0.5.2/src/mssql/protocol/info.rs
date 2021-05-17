use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::mssql::io::MssqlBufExt;

#[derive(Debug)]
pub(crate) struct Info {
    pub(crate) number: u32,
    pub(crate) state: u8,
    pub(crate) class: u8,
    pub(crate) message: String,
    pub(crate) server: String,
    pub(crate) procedure: String,
    pub(crate) line: u32,
}

impl Info {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let len = buf.get_u16_le();
        let mut data = buf.split_to(len as usize);

        let number = data.get_u32_le();
        let state = data.get_u8();
        let class = data.get_u8();
        let message = data.get_us_varchar()?;
        let server = data.get_b_varchar()?;
        let procedure = data.get_b_varchar()?;
        let line = data.get_u32_le();

        Ok(Self {
            number,
            state,
            class,
            message,
            server,
            procedure,
            line,
        })
    }
}
