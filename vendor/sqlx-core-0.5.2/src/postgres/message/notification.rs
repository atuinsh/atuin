use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::io::{BufExt, Decode};

#[derive(Debug)]
pub struct Notification {
    pub(crate) process_id: u32,
    pub(crate) channel: Bytes,
    pub(crate) payload: Bytes,
}

impl Decode<'_> for Notification {
    #[inline]
    fn decode_with(mut buf: Bytes, _: ()) -> Result<Self, Error> {
        let process_id = buf.get_u32();
        let channel = buf.get_bytes_nul()?;
        let payload = buf.get_bytes_nul()?;

        Ok(Self {
            process_id,
            channel,
            payload,
        })
    }
}

#[test]
fn test_decode_notification_response() {
    const NOTIFICATION_RESPONSE: &[u8] = b"\x34\x20\x10\x02TEST-CHANNEL\0THIS IS A TEST\0";

    let message = Notification::decode(Bytes::from(NOTIFICATION_RESPONSE)).unwrap();

    assert_eq!(message.process_id, 0x34201002);
    assert_eq!(&*message.channel, &b"TEST-CHANNEL"[..]);
    assert_eq!(&*message.payload, &b"THIS IS A TEST"[..]);
}
