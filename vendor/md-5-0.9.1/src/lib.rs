//! An implementation of the [MD5][1] cryptographic hash algorithm.
//!
//! # Usage
//!
//! ```rust
//! use md5::{Md5, Digest};
//! use hex_literal::hex;
//!
//! // create a Md5 hasher instance
//! let mut hasher = Md5::new();
//!
//! // process input message
//! hasher.update(b"hello world");
//!
//! // acquire hash digest in the form of GenericArray,
//! // which in this case is equivalent to [u8; 16]
//! let result = hasher.finalize();
//! assert_eq!(result[..], hex!("5eb63bbbe01eeed093cb22bb8f5acdc3"));
//! ```
//!
//! Also see [RustCrypto/hashes][2] readme.
//!
//! [1]: https://en.wikipedia.org/wiki/MD5
//! [2]: https://github.com/RustCrypto/hashes

#![no_std]
#![doc(html_logo_url = "https://raw.githubusercontent.com/RustCrypto/meta/master/logo_small.png")]
#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

#[cfg(feature = "asm")]
extern crate md5_asm as utils;

#[cfg(feature = "std")]
extern crate std;

#[cfg(not(feature = "asm"))]
mod utils;

pub use digest::{self, Digest};

use crate::utils::compress;

use block_buffer::BlockBuffer;
use digest::generic_array::typenum::{U16, U64};
use digest::generic_array::GenericArray;
use digest::{BlockInput, FixedOutputDirty, Reset, Update};

mod consts;

/// The MD5 hasher
#[derive(Clone)]
pub struct Md5 {
    length_bytes: u64,
    buffer: BlockBuffer<U64>,
    state: [u32; 4],
}

impl Default for Md5 {
    fn default() -> Self {
        Md5 {
            length_bytes: 0,
            buffer: Default::default(),
            state: consts::S0,
        }
    }
}

#[inline(always)]
fn convert(d: &GenericArray<u8, U64>) -> &[u8; 64] {
    #[allow(unsafe_code)]
    unsafe {
        &*(d.as_ptr() as *const [u8; 64])
    }
}

impl Md5 {
    #[inline]
    fn finalize_inner(&mut self) {
        let s = &mut self.state;
        let l = (self.length_bytes << 3) as u64;
        self.buffer.len64_padding_le(l, |d| compress(s, convert(d)));
    }
}

impl BlockInput for Md5 {
    type BlockSize = U64;
}

impl Update for Md5 {
    #[inline]
    fn update(&mut self, input: impl AsRef<[u8]>) {
        let input = input.as_ref();
        // Unlike Sha1 and Sha2, the length value in MD5 is defined as
        // the length of the message mod 2^64 - ie: integer overflow is OK.
        self.length_bytes = self.length_bytes.wrapping_add(input.len() as u64);
        let s = &mut self.state;
        self.buffer.input_block(input, |d| compress(s, convert(d)));
    }
}

impl FixedOutputDirty for Md5 {
    type OutputSize = U16;

    #[inline]
    fn finalize_into_dirty(&mut self, out: &mut GenericArray<u8, U16>) {
        self.finalize_inner();
        for (chunk, v) in out.chunks_exact_mut(4).zip(self.state.iter()) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
    }
}

impl Reset for Md5 {
    fn reset(&mut self) {
        self.state = consts::S0;
        self.length_bytes = 0;
        self.buffer.reset();
    }
}

opaque_debug::implement!(Md5);
digest::impl_write!(Md5);
