use crate::mssql::io::MssqlBufExt;
use bytes::{Buf, Bytes};

#[derive(Debug)]
pub(crate) struct Error {
    // The error number
    pub(crate) number: i32,

    // The error state, used as a modifier to the error number.
    pub(crate) state: u8,

    // The class (severity) of the error. A class of less than 10 indicates
    // an informational message.
    pub(crate) class: u8,

    // The message text length and message text using US_VARCHAR format.
    pub(crate) message: String,

    // The server name length and server name using B_VARCHAR format
    pub(crate) server: String,

    // The stored procedure name length and the stored procedure name using B_VARCHAR format
    pub(crate) procedure: String,

    // The line number in the SQL batch or stored procedure that caused the error. Line numbers
    // begin at 1. If the line number is not applicable to the message, the
    // value of LineNumber is 0.
    pub(crate) line: i32,
}

impl Error {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, crate::error::Error> {
        let len = buf.get_u16_le();
        let mut data = buf.split_to(len as usize);

        let number = data.get_i32_le();
        let state = data.get_u8();
        let class = data.get_u8();
        let message = data.get_us_varchar()?;
        let server = data.get_b_varchar()?;
        let procedure = data.get_b_varchar()?;
        let line = data.get_i32_le();

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
