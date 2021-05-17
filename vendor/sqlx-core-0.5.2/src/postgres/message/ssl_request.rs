use crate::io::Encode;

pub struct SslRequest;

impl Encode<'_> for SslRequest {
    #[inline]
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        buf.extend(&8_u32.to_be_bytes());
        buf.extend(&(((1234 << 16) | 5679) as u32).to_be_bytes());
    }
}

#[test]
fn test_encode_ssl_request() {
    const EXPECTED: &[u8] = b"\x00\x00\x00\x08\x04\xd2\x16/";

    let mut buf = Vec::new();
    SslRequest.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}
