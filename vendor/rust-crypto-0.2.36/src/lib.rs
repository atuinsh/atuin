// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![cfg_attr(feature = "with-bench", feature(test))]

extern crate rand;
extern crate rustc_serialize as serialize;
extern crate time;
extern crate libc;

#[cfg(all(test, feature = "with-bench"))]
extern crate test;

pub mod aead;
pub mod aes;
pub mod aes_gcm;
pub mod aessafe;
pub mod bcrypt;
pub mod bcrypt_pbkdf;
pub mod blake2b;
pub mod blake2s;
pub mod blockmodes;
pub mod blowfish;
pub mod buffer;
pub mod chacha20;
pub mod chacha20poly1305;
mod cryptoutil;
pub mod curve25519;
pub mod digest;
pub mod ed25519;
pub mod fortuna;
pub mod ghash;
pub mod hc128;
pub mod hmac;
pub mod hkdf;
pub mod mac;
pub mod md5;
pub mod pbkdf2;
pub mod poly1305;
pub mod rc4;
pub mod ripemd160;
pub mod salsa20;
pub mod scrypt;
pub mod sha1;
pub mod sha2;
pub mod sha3;
mod simd;
pub mod sosemanuk;
mod step_by;
pub mod symmetriccipher;
pub mod util;
pub mod whirlpool;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub mod aesni;
