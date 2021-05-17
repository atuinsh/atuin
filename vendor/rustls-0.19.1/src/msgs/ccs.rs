use crate::msgs::codec::{Codec, Reader};

#[derive(Debug)]
pub struct ChangeCipherSpecPayload;

impl Codec for ChangeCipherSpecPayload {
    fn encode(&self, bytes: &mut Vec<u8>) {
        1u8.encode(bytes);
    }

    fn read(r: &mut Reader) -> Option<ChangeCipherSpecPayload> {
        let typ = u8::read(r)?;

        if typ == 1 && !r.any_left() {
            Some(ChangeCipherSpecPayload {})
        } else {
            None
        }
    }
}

impl ChangeCipherSpecPayload {
    pub fn length(&self) -> usize {
        1
    }
}
