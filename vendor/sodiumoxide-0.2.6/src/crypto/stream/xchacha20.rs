//! `xchacha20`. The same construction as `xsalsa20` but using
//! `chacha20` instead of `salsa20` as the underlying stream cipher.
//! This cipher is conjectured to meet the standard notion of
//! unpredictability.

use crypto::nonce::gen_random_nonce;
use ffi::{
    crypto_stream_xchacha20, crypto_stream_xchacha20_KEYBYTES, crypto_stream_xchacha20_NONCEBYTES,
    crypto_stream_xchacha20_xor, crypto_stream_xchacha20_xor_ic,
};

stream_module!(
    crypto_stream_xchacha20,
    crypto_stream_xchacha20_xor,
    crypto_stream_xchacha20_xor_ic,
    crypto_stream_xchacha20_KEYBYTES as usize,
    crypto_stream_xchacha20_NONCEBYTES as usize
);

/// `gen_nonce` randomly generates a nonce
///
/// THREAD SAFETY: `gen_nonce()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_nonce() -> Nonce {
    gen_random_nonce()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_nonce_length() {
        assert_eq!(192 / 8, gen_nonce().as_ref().len());
    }
}
