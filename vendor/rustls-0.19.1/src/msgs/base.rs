use crate::key;
use crate::msgs::codec;
use crate::msgs::codec::{Codec, Reader};
/// An externally length'd payload
#[derive(Debug, Clone, PartialEq)]
pub struct Payload(pub Vec<u8>);

impl Codec for Payload {
    fn encode(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.0);
    }

    fn read(r: &mut Reader) -> Option<Payload> {
        Some(Payload(r.rest().to_vec()))
    }
}

impl Payload {
    pub fn new(bytes: Vec<u8>) -> Payload {
        Payload(bytes)
    }

    pub fn empty() -> Payload {
        Payload::new(Vec::new())
    }
}

impl Codec for key::Certificate {
    fn encode(&self, bytes: &mut Vec<u8>) {
        codec::u24(self.0.len() as u32).encode(bytes);
        bytes.extend_from_slice(&self.0);
    }

    fn read(r: &mut Reader) -> Option<key::Certificate> {
        let len = codec::u24::read(r)?.0 as usize;
        let mut sub = r.sub(len)?;
        let body = sub.rest().to_vec();
        Some(key::Certificate(body))
    }
}

/// An arbitrary, unknown-content, u24-length-prefixed payload
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadU24(pub Vec<u8>);

impl PayloadU24 {
    pub fn new(bytes: Vec<u8>) -> PayloadU24 {
        PayloadU24(bytes)
    }
}

impl Codec for PayloadU24 {
    fn encode(&self, bytes: &mut Vec<u8>) {
        codec::u24(self.0.len() as u32).encode(bytes);
        bytes.extend_from_slice(&self.0);
    }

    fn read(r: &mut Reader) -> Option<PayloadU24> {
        let len = codec::u24::read(r)?.0 as usize;
        let mut sub = r.sub(len)?;
        let body = sub.rest().to_vec();
        Some(PayloadU24(body))
    }
}

/// An arbitrary, unknown-content, u16-length-prefixed payload
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadU16(pub Vec<u8>);

impl PayloadU16 {
    pub fn new(bytes: Vec<u8>) -> PayloadU16 {
        PayloadU16(bytes)
    }

    pub fn empty() -> PayloadU16 {
        PayloadU16::new(Vec::new())
    }

    pub fn encode_slice(slice: &[u8], bytes: &mut Vec<u8>) {
        (slice.len() as u16).encode(bytes);
        bytes.extend_from_slice(slice);
    }
}

impl Codec for PayloadU16 {
    fn encode(&self, bytes: &mut Vec<u8>) {
        Self::encode_slice(&self.0, bytes);
    }

    fn read(r: &mut Reader) -> Option<PayloadU16> {
        let len = u16::read(r)? as usize;
        let mut sub = r.sub(len)?;
        let body = sub.rest().to_vec();
        Some(PayloadU16(body))
    }
}

/// An arbitrary, unknown-content, u8-length-prefixed payload
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadU8(pub Vec<u8>);

impl PayloadU8 {
    pub fn new(bytes: Vec<u8>) -> PayloadU8 {
        PayloadU8(bytes)
    }

    pub fn empty() -> PayloadU8 {
        PayloadU8(Vec::new())
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl Codec for PayloadU8 {
    fn encode(&self, bytes: &mut Vec<u8>) {
        (self.0.len() as u8).encode(bytes);
        bytes.extend_from_slice(&self.0);
    }

    fn read(r: &mut Reader) -> Option<PayloadU8> {
        let len = u8::read(r)? as usize;
        let mut sub = r.sub(len)?;
        let body = sub.rest().to_vec();
        Some(PayloadU8(body))
    }
}
