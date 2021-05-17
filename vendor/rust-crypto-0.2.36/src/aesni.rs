// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use aes::KeySize;
use aes::KeySize::{KeySize128, KeySize192, KeySize256};
use symmetriccipher::{BlockEncryptor, BlockDecryptor};
use util::supports_aesni;

#[derive(Copy)]
pub struct AesNiEncryptor {
    rounds: u8,
    round_keys: [u8; 240]
}

impl Clone for AesNiEncryptor { fn clone(&self) -> AesNiEncryptor { *self } }

#[derive(Copy)]
pub struct AesNiDecryptor {
    rounds: u8,
    round_keys: [u8; 240]
}

impl Clone for AesNiDecryptor { fn clone(&self) -> AesNiDecryptor { *self } }

/// The number of rounds as well as a function to setup an appropriately sized key.
type RoundSetupInfo = (u8, fn(&[u8], KeyType, &mut [u8]));

impl AesNiEncryptor {
    pub fn new(key_size: KeySize, key: &[u8]) -> AesNiEncryptor {
        if !supports_aesni() {
            panic!("AES-NI not supported on this architecture. If you are \
                using the MSVC toolchain, this is because the AES-NI method's \
                have not been ported, yet");
        }
        let (rounds, setup_function): RoundSetupInfo = match key_size {
            KeySize128 => (10, setup_working_key_aesni_128),
            KeySize192 => (12, setup_working_key_aesni_192),
            KeySize256 => (14, setup_working_key_aesni_256)
        };
        let mut e = AesNiEncryptor {
            rounds: rounds,
            round_keys: [0u8; 240]
        };
        setup_function(key, KeyType::Encryption, &mut e.round_keys[0..size(e.rounds)]);
        e
    }
}

impl AesNiDecryptor {
    pub fn new(key_size: KeySize, key: &[u8]) -> AesNiDecryptor {
        if !supports_aesni() {
            panic!("AES-NI not supported on this architecture. If you are \
                using the MSVC toolchain, this is because the AES-NI method's \
                have not been ported, yet");
        }
        let (rounds, setup_function): RoundSetupInfo = match key_size {
            KeySize128 => (10, setup_working_key_aesni_128),
            KeySize192 => (12, setup_working_key_aesni_192),
            KeySize256 => (14, setup_working_key_aesni_256)
        };
        let mut d = AesNiDecryptor {
            rounds: rounds,
            round_keys: [0u8; 240]
        };
        setup_function(key, KeyType::Decryption, &mut d.round_keys[0..size(d.rounds)]);
        d
    }

}

impl BlockEncryptor for AesNiEncryptor {
    fn block_size(&self) -> usize { 16 }
    fn encrypt_block(&self, input: &[u8], output: &mut [u8]) {
        encrypt_block_aesni(self.rounds, input, &self.round_keys[0..size(self.rounds)], output);
    }
}

impl BlockDecryptor for AesNiDecryptor {
    fn block_size(&self) -> usize { 16 }
    fn decrypt_block(&self, input: &[u8], output: &mut [u8]) {
        decrypt_block_aesni(self.rounds, input, &self.round_keys[0..size(self.rounds)], output);
    }
}

enum KeyType {
    Encryption,
    Decryption
}

#[inline(always)]
fn size(rounds: u8) -> usize { 16 * ((rounds as usize) + 1) }

extern {
    fn rust_crypto_aesni_aesimc(round_keys: *mut u8);
    fn rust_crypto_aesni_setup_working_key_128(key: *const u8, round_key: *mut u8);
    fn rust_crypto_aesni_setup_working_key_192(key: *const u8, round_key: *mut u8);
    fn rust_crypto_aesni_setup_working_key_256(key: *const u8, round_key: *mut u8);
    fn rust_crypto_aesni_encrypt_block(
            rounds: u8,
            input: *const u8,
            round_keys: *const u8,
            output: *mut u8);
    fn rust_crypto_aesni_decrypt_block(
            rounds: u8,
            input: *const u8,
            round_keys: *const u8,
            output: *mut u8);
}

fn setup_working_key_aesni_128(key: &[u8], key_type: KeyType, round_key: &mut [u8]) {
    unsafe {
        rust_crypto_aesni_setup_working_key_128(key.as_ptr(), round_key.as_mut_ptr());

        match key_type {
            KeyType::Decryption => {
                // range of rounds keys from #1 to #9; skip the first and last key
                for i in 1..10 {
                    rust_crypto_aesni_aesimc(round_key.get_unchecked_mut(16 * i));
                }
            }
            KeyType::Encryption => { /* nothing more to do */ }
        }
    }
}

fn setup_working_key_aesni_192(key: &[u8], key_type: KeyType, round_key: &mut [u8]) {
    unsafe {
        rust_crypto_aesni_setup_working_key_192(key.as_ptr(), round_key.as_mut_ptr());

        match key_type {
            KeyType::Decryption => {
                // range of rounds keys from #1 to #11; skip the first and last key
                for i in 1..12 {
                    rust_crypto_aesni_aesimc(round_key.get_unchecked_mut(16 * i));
                }
            }
            KeyType::Encryption => { /* nothing more to do */ }
        }
    }
}

fn setup_working_key_aesni_256(key: &[u8], key_type: KeyType, round_key: &mut [u8]) {
    unsafe {
        rust_crypto_aesni_setup_working_key_256(key.as_ptr(), round_key.as_mut_ptr());

        match key_type {
            KeyType::Decryption => {
                // range of rounds keys from #1 to #13; skip the first and last key
                for i in 1..14 {
                    rust_crypto_aesni_aesimc(round_key.get_unchecked_mut(16 * i));
                }
            }
            KeyType::Encryption => { /* nothing more to do */ }
        }
    }
}

fn encrypt_block_aesni(rounds: u8, input: &[u8], round_keys: &[u8], output: &mut [u8]) {
    unsafe {
        rust_crypto_aesni_encrypt_block(
                rounds,
                input.as_ptr(),
                round_keys.as_ptr(),
                output.as_mut_ptr());
    }
}

fn decrypt_block_aesni(rounds: u8, input: &[u8], round_keys: &[u8], output: &mut [u8]) {
    unsafe {
        rust_crypto_aesni_decrypt_block(
                rounds as u8,
                input.as_ptr(),
                round_keys.get_unchecked(round_keys.len() - 16),
                output.as_mut_ptr());
    }
}
