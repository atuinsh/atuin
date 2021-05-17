//! `HMAC-SHA-512-256`, i.e., the first 256 bits of
//! `HMAC-SHA-512`.  `HMAC-SHA-512-256` is conjectured to meet the standard notion
//! of unforgeability.

use ffi::{
    crypto_auth_hmacsha512256, crypto_auth_hmacsha512256_BYTES, crypto_auth_hmacsha512256_KEYBYTES,
    crypto_auth_hmacsha512256_final, crypto_auth_hmacsha512256_init,
    crypto_auth_hmacsha512256_state, crypto_auth_hmacsha512256_update,
    crypto_auth_hmacsha512256_verify,
};

auth_module!(
    crypto_auth_hmacsha512256,
    crypto_auth_hmacsha512256_verify,
    crypto_auth_hmacsha512256_KEYBYTES as usize,
    crypto_auth_hmacsha512256_BYTES as usize
);

auth_state!(
    crypto_auth_hmacsha512256_state,
    crypto_auth_hmacsha512256_init,
    crypto_auth_hmacsha512256_update,
    crypto_auth_hmacsha512256_final,
    crypto_auth_hmacsha512256_BYTES as usize
);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_vector_1() {
        // corresponding to tests/auth.c from NaCl
        // "Test Case 2" from RFC 4231
        let key = Key([
            74, 101, 102, 101, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ]);
        let c = [
            0x77, 0x68, 0x61, 0x74, 0x20, 0x64, 0x6f, 0x20, 0x79, 0x61, 0x20, 0x77, 0x61, 0x6e,
            0x74, 0x20, 0x66, 0x6f, 0x72, 0x20, 0x6e, 0x6f, 0x74, 0x68, 0x69, 0x6e, 0x67, 0x3f,
        ];

        let a_expected = [
            0x16, 0x4b, 0x7a, 0x7b, 0xfc, 0xf8, 0x19, 0xe2, 0xe3, 0x95, 0xfb, 0xe7, 0x3b, 0x56,
            0xe0, 0xa3, 0x87, 0xbd, 0x64, 0x22, 0x2e, 0x83, 0x1f, 0xd6, 0x10, 0x27, 0x0c, 0xd7,
            0xea, 0x25, 0x05, 0x54,
        ];

        let Tag(a) = authenticate(&c, &key);
        assert!(a == a_expected);
    }

    #[test]
    fn test_vector_state_1() {
        // corresponding to tests/auth.c from NaCl
        // "Test Case 2" from RFC 4231
        let key = [
            74, 101, 102, 101, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        let c = [
            0x77, 0x68, 0x61, 0x74, 0x20, 0x64, 0x6f, 0x20, 0x79, 0x61, 0x20, 0x77, 0x61, 0x6e,
            0x74, 0x20, 0x66, 0x6f, 0x72, 0x20, 0x6e, 0x6f, 0x74, 0x68, 0x69, 0x6e, 0x67, 0x3f,
        ];

        let a_expected = [
            0x16, 0x4b, 0x7a, 0x7b, 0xfc, 0xf8, 0x19, 0xe2, 0xe3, 0x95, 0xfb, 0xe7, 0x3b, 0x56,
            0xe0, 0xa3, 0x87, 0xbd, 0x64, 0x22, 0x2e, 0x83, 0x1f, 0xd6, 0x10, 0x27, 0x0c, 0xd7,
            0xea, 0x25, 0x05, 0x54,
        ];

        let mut state = State::init(&key);
        state.update(&c);
        let Tag(a) = state.finalize();
        assert!(a == a_expected);
    }
}
