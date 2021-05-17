use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::io::BufExt;

pub trait MssqlBufExt: Buf {
    fn get_utf16_str(&mut self, n: usize) -> Result<String, Error>;

    fn get_b_varchar(&mut self) -> Result<String, Error>;

    fn get_us_varchar(&mut self) -> Result<String, Error>;

    fn get_b_varbyte(&mut self) -> Bytes;
}

impl MssqlBufExt for Bytes {
    fn get_utf16_str(&mut self, mut n: usize) -> Result<String, Error> {
        let mut raw = Vec::with_capacity(n * 2);

        while n > 0 {
            let ch = self.get_u16_le();
            raw.push(ch);
            n -= 1;
        }

        String::from_utf16(&raw).map_err(Error::protocol)
    }

    fn get_b_varchar(&mut self) -> Result<String, Error> {
        let size = self.get_u8();
        self.get_utf16_str(size as usize)
    }

    fn get_us_varchar(&mut self) -> Result<String, Error> {
        let size = self.get_u16_le();
        self.get_utf16_str(size as usize)
    }

    fn get_b_varbyte(&mut self) -> Bytes {
        let size = self.get_u8();
        self.get_bytes(size as usize)
    }
}
