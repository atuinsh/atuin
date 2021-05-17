//! Generic implementation of Hash-based Message Authentication Code (HMAC).
//!
//! To use it you'll need a cryptographic hash function implementation from
//! RustCrypto project. You can either import specific crate (e.g. `sha2`), or
//! meta-crate `crypto-hashes` which reexport all related crates.
//!
//! # Usage
//! Let us demonstrate how to use HMAC using SHA256 as an example.
//!
//! To get the authentication code:
//!
//! ```rust
//! use sha2::Sha256;
//! use hmac::{Hmac, Mac, NewMac};
//!
//! // Create alias for HMAC-SHA256
//! type HmacSha256 = Hmac<Sha256>;
//!
//! // Create HMAC-SHA256 instance which implements `Mac` trait
//! let mut mac = HmacSha256::new_varkey(b"my secret and secure key")
//!     .expect("HMAC can take key of any size");
//! mac.update(b"input message");
//!
//! // `result` has type `Output` which is a thin wrapper around array of
//! // bytes for providing constant time equality check
//! let result = mac.finalize();
//! // To get underlying array use `into_bytes` method, but be careful, since
//! // incorrect use of the code value may permit timing attacks which defeat
//! // the security provided by the `Output`
//! let code_bytes = result.into_bytes();
//! ```
//!
//! To verify the message:
//!
//! ```rust
//! # use sha2::Sha256;
//! # use hmac::{Hmac, Mac, NewMac};
//! # type HmacSha256 = Hmac<Sha256>;
//! let mut mac = HmacSha256::new_varkey(b"my secret and secure key")
//!     .expect("HMAC can take key of any size");
//!
//! mac.update(b"input message");
//!
//! # let code_bytes = mac.clone().finalize().into_bytes();
//! // `verify` will return `Ok(())` if code is correct, `Err(MacError)` otherwise
//! mac.verify(&code_bytes).unwrap();
//! ```
//!
//! # Block and input sizes
//! Usually it is assumed that block size is larger than output size, due to the
//! generic nature of the implementation this edge case must be handled as well
//! to remove potential panic scenario. This is done by truncating hash output
//! to the hash block size if needed.

#![no_std]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/RustCrypto/meta/master/logo.svg",
    html_favicon_url = "https://raw.githubusercontent.com/RustCrypto/meta/master/logo.svg"
)]
#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

#[cfg(feature = "std")]
extern crate std;

pub use crypto_mac::{self, Mac, NewMac};
pub use digest;

use core::{cmp::min, fmt};
use crypto_mac::{
    generic_array::{sequence::GenericSequence, ArrayLength, GenericArray},
    InvalidKeyLength, Output,
};
use digest::{BlockInput, FixedOutput, Reset, Update};

const IPAD: u8 = 0x36;
const OPAD: u8 = 0x5C;

/// The `Hmac` struct represents an HMAC using a given hash function `D`.
pub struct Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone,
    D::BlockSize: ArrayLength<u8>,
{
    digest: D,
    i_key_pad: GenericArray<u8, D::BlockSize>,
    opad_digest: D,
}

impl<D> Clone for Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone,
    D::BlockSize: ArrayLength<u8>,
{
    fn clone(&self) -> Hmac<D> {
        Hmac {
            digest: self.digest.clone(),
            i_key_pad: self.i_key_pad.clone(),
            opad_digest: self.opad_digest.clone(),
        }
    }
}

impl<D> fmt::Debug for Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone + fmt::Debug,
    D::BlockSize: ArrayLength<u8>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hmac")
            .field("digest", &self.digest)
            .field("i_key_pad", &self.i_key_pad)
            .field("opad_digest", &self.opad_digest)
            .finish()
    }
}

impl<D> NewMac for Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone,
    D::BlockSize: ArrayLength<u8>,
    D::OutputSize: ArrayLength<u8>,
{
    type KeySize = D::BlockSize;

    fn new(key: &GenericArray<u8, Self::KeySize>) -> Self {
        Self::new_varkey(key.as_slice()).unwrap()
    }

    #[inline]
    fn new_varkey(key: &[u8]) -> Result<Self, InvalidKeyLength> {
        let mut hmac = Self {
            digest: Default::default(),
            i_key_pad: GenericArray::generate(|_| IPAD),
            opad_digest: Default::default(),
        };

        let mut opad = GenericArray::<u8, D::BlockSize>::generate(|_| OPAD);
        debug_assert!(hmac.i_key_pad.len() == opad.len());

        // The key that Hmac processes must be the same as the block size of the
        // underlying Digest. If the provided key is smaller than that, we just
        // pad it with zeros. If its larger, we hash it and then pad it with
        // zeros.
        if key.len() <= hmac.i_key_pad.len() {
            for (k_idx, k_itm) in key.iter().enumerate() {
                hmac.i_key_pad[k_idx] ^= *k_itm;
                opad[k_idx] ^= *k_itm;
            }
        } else {
            let mut digest = D::default();
            digest.update(key);
            let output = digest.finalize_fixed();
            // `n` is calculated at compile time and will equal
            // D::OutputSize. This is used to ensure panic-free code
            let n = min(output.len(), hmac.i_key_pad.len());
            for idx in 0..n {
                hmac.i_key_pad[idx] ^= output[idx];
                opad[idx] ^= output[idx];
            }
        }

        hmac.digest.update(&hmac.i_key_pad);
        hmac.opad_digest.update(&opad);

        Ok(hmac)
    }
}

impl<D> Mac for Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone,
    D::BlockSize: ArrayLength<u8>,
    D::OutputSize: ArrayLength<u8>,
{
    type OutputSize = D::OutputSize;

    #[inline]
    fn update(&mut self, data: &[u8]) {
        self.digest.update(data);
    }

    #[inline]
    fn finalize(self) -> Output<Self> {
        let mut opad_digest = self.opad_digest.clone();
        let hash = self.digest.finalize_fixed();
        opad_digest.update(&hash);
        Output::new(opad_digest.finalize_fixed())
    }

    #[inline]
    fn reset(&mut self) {
        self.digest.reset();
        self.digest.update(&self.i_key_pad);
    }
}

#[cfg(feature = "std")]
impl<D> std::io::Write for Hmac<D>
where
    D: Update + BlockInput + FixedOutput + Reset + Default + Clone,
    D::BlockSize: ArrayLength<u8>,
    D::OutputSize: ArrayLength<u8>,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Mac::update(self, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
