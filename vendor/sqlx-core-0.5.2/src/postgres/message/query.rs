use crate::io::{BufMutExt, Encode};

#[derive(Debug)]
pub struct Query<'a>(pub &'a str);

impl Encode<'_> for Query<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        let len = 4 + self.0.len() + 1;

        buf.reserve(len + 1);
        buf.push(b'Q');
        buf.extend(&(len as i32).to_be_bytes());
        buf.put_str_nul(self.0);
    }
}

#[test]
fn test_encode_query() {
    const EXPECTED: &[u8] = b"Q\0\0\0\rSELECT 1\0";

    let mut buf = Vec::new();
    let m = Query("SELECT 1");

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}
