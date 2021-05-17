pub trait MssqlBufMutExt {
    fn put_b_varchar(&mut self, s: &str);
    fn put_utf16_str(&mut self, s: &str);
}

impl MssqlBufMutExt for Vec<u8> {
    fn put_utf16_str(&mut self, s: &str) {
        let mut enc = s.encode_utf16();
        while let Some(ch) = enc.next() {
            self.extend_from_slice(&ch.to_le_bytes());
        }
    }

    fn put_b_varchar(&mut self, s: &str) {
        self.extend(&(s.len() as u8).to_le_bytes());
        self.put_utf16_str(s);
    }
}
