//! `crypto_stream_salsa20` (Salsa20/20), a particular cipher specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html), Section 7.  This
//! cipher is conjectured to meet the standard notion of unpredictability.

use ffi::{
    crypto_stream_salsa20, crypto_stream_salsa20_KEYBYTES, crypto_stream_salsa20_NONCEBYTES,
    crypto_stream_salsa20_xor, crypto_stream_salsa20_xor_ic,
};

stream_module!(
    crypto_stream_salsa20,
    crypto_stream_salsa20_xor,
    crypto_stream_salsa20_xor_ic,
    crypto_stream_salsa20_KEYBYTES as usize,
    crypto_stream_salsa20_NONCEBYTES as usize
);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_vector_1() {
        // corresponding to tests/stream2.c and tests/stream6.cpp from NaCl
        use crypto::hash::sha256::{hash, Digest};
        let secondkey = Key([
            0xdc, 0x90, 0x8d, 0xda, 0x0b, 0x93, 0x44, 0xa9, 0x53, 0x62, 0x9b, 0x73, 0x38, 0x20,
            0x77, 0x88, 0x80, 0xf3, 0xce, 0xb4, 0x21, 0xbb, 0x61, 0xb9, 0x1c, 0xbd, 0x4c, 0x3e,
            0x66, 0x25, 0x6c, 0xe4,
        ]);
        let noncesuffix = Nonce([0x82, 0x19, 0xe0, 0x03, 0x6b, 0x7a, 0x0b, 0x37]);
        let output = stream(4_194_304, &noncesuffix, &secondkey);
        let digest_expected = [
            0x66, 0x2b, 0x9d, 0x0e, 0x34, 0x63, 0x02, 0x91, 0x56, 0x06, 0x9b, 0x12, 0xf9, 0x18,
            0x69, 0x1a, 0x98, 0xf7, 0xdf, 0xb2, 0xca, 0x03, 0x93, 0xc9, 0x6b, 0xbf, 0xc6, 0xb1,
            0xfb, 0xd6, 0x30, 0xa2,
        ];
        let Digest(digest) = hash(&output);
        assert!(digest == digest_expected);
    }
}
