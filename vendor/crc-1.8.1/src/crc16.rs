#[cfg(feature = "std")]
use std::hash::Hasher;
#[cfg(not(feature = "std"))]
use core::hash::Hasher;

pub use util::make_table_crc16 as make_table;

include!(concat!(env!("OUT_DIR"), "/crc16_constants.rs"));

pub struct Digest {
    table: [u16; 256],
    initial: u16,
    value: u16
}

pub trait Hasher16 {
    fn reset(&mut self);
    fn write(&mut self, bytes: &[u8]);
    fn sum16(&self) -> u16;
}

pub fn update(mut value: u16, table: &[u16; 256], bytes: &[u8]) -> u16 {
    value = !value;
    for &i in bytes.iter() {
        value = table[((value as u8) ^ i) as usize] ^ (value >> 8)
    }
    !value
}

pub fn checksum_x25(bytes: &[u8]) -> u16 {
    return update(0, &X25_TABLE, bytes);
}

pub fn checksum_usb(bytes: &[u8]) -> u16 {
    return update(0, &USB_TABLE, bytes);
}

impl Digest {
    pub fn new(poly: u16) -> Digest {
        Digest {
            table: make_table(poly),
            initial: 0,
            value: 0
        }
    }

    pub fn new_with_initial(poly: u16, initial: u16) -> Digest {
        Digest {
            table: make_table(poly),
            initial: initial,
            value: initial
        }
    }
}

impl Hasher16 for Digest {
    fn reset(&mut self) {
        self.value = self.initial;
    }
    fn write(&mut self, bytes: &[u8]) {
        self.value = update(self.value, &self.table, bytes);
    }
    fn sum16(&self) -> u16 {
        self.value
    }
}

/// Implementation of std::hash::Hasher so that types which #[derive(Hash)] can hash with Digest.
impl Hasher for Digest {
    fn write(&mut self, bytes: &[u8]) {
        Hasher16::write(self, bytes);
    }

    fn finish(&self) -> u64 {
        self.sum16() as u64
    }
}
