use bitflags::bitflags;
use bytes::{Buf, Bytes};

use crate::error::Error;

#[derive(Debug)]
pub(crate) struct Done {
    pub(crate) status: Status,

    // The token of the current SQL statement. The token value is provided and controlled by the
    // application layer, which utilizes TDS. The TDS layer does not evaluate the value.
    cursor_command: u16,

    // The count of rows that were affected by the SQL statement. The value of DoneRowCount is
    // valid if the value of Status includes DONE_COUNT.
    pub(crate) affected_rows: u64, // NOTE: u32 before TDS 7.2
}

impl Done {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let status = Status::from_bits_truncate(buf.get_u16_le());
        let cursor_command = buf.get_u16_le();
        let affected_rows = buf.get_u64_le();

        Ok(Self {
            affected_rows,
            status,
            cursor_command,
        })
    }
}

bitflags! {
    pub struct Status: u16 {
        // This DONEINPROC message is not the final DONE/DONEPROC/DONEINPROC message in
        // the response; more data streams are to follow.
        const DONE_MORE = 0x0001;

        // An error occurred on the current SQL statement or execution of a stored procedure was
        // interrupted. A preceding ERROR token SHOULD be sent when this bit is set.
        const DONE_ERROR = 0x0002;

        // A transaction is in progress.
        const DONE_INXACT = 0x0004;

        // The DoneRowCount value is valid. This is used to distinguish between a valid value of 0
        // for DoneRowCount or just an initialized variable.
        const DONE_COUNT = 0x0010;

        // Used in place of DONE_ERROR when an error occurred on the current SQL statement that is
        // severe enough to require the result set, if any, to be discarded.
        const DONE_SRVERROR = 0x0100;
    }
}
