use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::io::Decode;
use crate::mysql::protocol::response::Status;
use crate::mysql::protocol::Capabilities;

/// Marks the end of a result set, returning status and warnings.
///
/// # Note
///
/// The EOF packet is deprecated as of MySQL 5.7.5. SQLx only uses this packet for MySQL
/// prior MySQL versions.
#[derive(Debug)]
pub struct EofPacket {
    pub warnings: u16,
    pub status: Status,
}

impl Decode<'_, Capabilities> for EofPacket {
    fn decode_with(mut buf: Bytes, _: Capabilities) -> Result<Self, Error> {
        let header = buf.get_u8();
        if header != 0xfe {
            return Err(err_protocol!(
                "expected 0xfe (EOF_Packet) but found 0x{:x}",
                header
            ));
        }

        let warnings = buf.get_u16_le();
        let status = Status::from_bits_truncate(buf.get_u16_le());

        Ok(Self { status, warnings })
    }
}
