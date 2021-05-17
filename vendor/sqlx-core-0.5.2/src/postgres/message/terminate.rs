use crate::io::Encode;

pub struct Terminate;

impl Encode<'_> for Terminate {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        buf.push(b'X');
        buf.extend(&4_u32.to_be_bytes());
    }
}
