use std::ops::{Deref, DerefMut};

use bytes::Bytes;

use crate::error::Error;
use crate::io::{Decode, Encode};
use crate::mysql::protocol::response::{EofPacket, OkPacket};
use crate::mysql::protocol::Capabilities;

#[derive(Debug)]
pub struct Packet<T>(pub(crate) T);

impl<'en, 'stream, T> Encode<'stream, (Capabilities, &'stream mut u8)> for Packet<T>
where
    T: Encode<'en, Capabilities>,
{
    fn encode_with(
        &self,
        buf: &mut Vec<u8>,
        (capabilities, sequence_id): (Capabilities, &'stream mut u8),
    ) {
        // reserve space to write the prefixed length
        let offset = buf.len();
        buf.extend(&[0_u8; 4]);

        // encode the payload
        self.0.encode_with(buf, capabilities);

        // determine the length of the encoded payload
        // and write to our reserved space
        let len = buf.len() - offset - 4;
        let header = &mut buf[offset..];

        // FIXME: Support larger packets
        assert!(len < 0xFF_FF_FF);

        header[..4].copy_from_slice(&(len as u32).to_le_bytes());
        header[3] = *sequence_id;

        *sequence_id = sequence_id.wrapping_add(1);
    }
}

impl Packet<Bytes> {
    pub(crate) fn decode<'de, T>(self) -> Result<T, Error>
    where
        T: Decode<'de, ()>,
    {
        self.decode_with(())
    }

    pub(crate) fn decode_with<'de, T, C>(self, context: C) -> Result<T, Error>
    where
        T: Decode<'de, C>,
    {
        T::decode_with(self.0, context)
    }

    pub(crate) fn ok(self) -> Result<OkPacket, Error> {
        self.decode()
    }

    pub(crate) fn eof(self, capabilities: Capabilities) -> Result<EofPacket, Error> {
        if capabilities.contains(Capabilities::DEPRECATE_EOF) {
            let ok = self.ok()?;

            Ok(EofPacket {
                warnings: ok.warnings,
                status: ok.status,
            })
        } else {
            self.decode_with(capabilities)
        }
    }
}

impl Deref for Packet<Bytes> {
    type Target = Bytes;

    fn deref(&self) -> &Bytes {
        &self.0
    }
}

impl DerefMut for Packet<Bytes> {
    fn deref_mut(&mut self) -> &mut Bytes {
        &mut self.0
    }
}
