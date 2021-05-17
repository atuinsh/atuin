//! This crate provides trait for Message Authentication Code (MAC) algorithms.

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/RustCrypto/meta/master/logo.svg",
    html_favicon_url = "https://raw.githubusercontent.com/RustCrypto/meta/master/logo.svg"
)]
#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "cipher")]
pub use cipher;
#[cfg(feature = "cipher")]
use cipher::{BlockCipher, NewBlockCipher};

#[cfg(feature = "dev")]
#[cfg_attr(docsrs, doc(cfg(feature = "dev")))]
pub mod dev;

mod errors;

pub use crate::errors::{InvalidKeyLength, MacError};
pub use generic_array::{self, typenum::consts};

use generic_array::typenum::Unsigned;
use generic_array::{ArrayLength, GenericArray};
use subtle::{Choice, ConstantTimeEq};

/// Key for an algorithm that implements [`NewMac`].
pub type Key<M> = GenericArray<u8, <M as NewMac>::KeySize>;

/// Instantiate a [`Mac`] algorithm.
pub trait NewMac: Sized {
    /// Key size in bytes with which cipher guaranteed to be initialized.
    type KeySize: ArrayLength<u8>;

    /// Initialize new MAC instance from key with fixed size.
    fn new(key: &Key<Self>) -> Self;

    /// Initialize new MAC instance from key with variable size.
    ///
    /// Default implementation will accept only keys with length equal to
    /// `KeySize`, but some MACs can accept range of key lengths.
    fn new_varkey(key: &[u8]) -> Result<Self, InvalidKeyLength> {
        if key.len() != Self::KeySize::to_usize() {
            Err(InvalidKeyLength)
        } else {
            Ok(Self::new(GenericArray::from_slice(key)))
        }
    }
}

/// The [`Mac`] trait defines methods for a Message Authentication algorithm.
pub trait Mac: Clone {
    /// Output size of the [[`Mac`]]
    type OutputSize: ArrayLength<u8>;

    /// Update MAC state with the given data.
    fn update(&mut self, data: &[u8]);

    /// Reset [`Mac`] instance.
    fn reset(&mut self);

    /// Obtain the result of a [`Mac`] computation as a [`Output`] and consume
    /// [`Mac`] instance.
    fn finalize(self) -> Output<Self>;

    /// Obtain the result of a [`Mac`] computation as a [`Output`] and reset
    /// [`Mac`] instance.
    fn finalize_reset(&mut self) -> Output<Self> {
        let res = self.clone().finalize();
        self.reset();
        res
    }

    /// Check if tag/code value is correct for the processed input.
    fn verify(self, tag: &[u8]) -> Result<(), MacError> {
        let choice = self.finalize().bytes.ct_eq(tag);

        if choice.unwrap_u8() == 1 {
            Ok(())
        } else {
            Err(MacError)
        }
    }
}

/// [`Output`] is a thin wrapper around bytes array which provides a safe `Eq`
/// implementation that runs in a fixed time.
#[derive(Clone)]
pub struct Output<M: Mac> {
    bytes: GenericArray<u8, M::OutputSize>,
}

impl<M: Mac> Output<M> {
    /// Create a new MAC [`Output`].
    pub fn new(bytes: GenericArray<u8, M::OutputSize>) -> Output<M> {
        Output { bytes }
    }

    /// Get the MAC tag/code value as a byte array.
    ///
    /// Be very careful using this method, since incorrect use of the tag value
    /// may permit timing attacks which defeat the security provided by the
    /// [`Mac`] trait.
    pub fn into_bytes(self) -> GenericArray<u8, M::OutputSize> {
        self.bytes
    }
}

impl<M: Mac> ConstantTimeEq for Output<M> {
    fn ct_eq(&self, other: &Self) -> Choice {
        self.bytes.ct_eq(&other.bytes)
    }
}

impl<M: Mac> PartialEq for Output<M> {
    fn eq(&self, x: &Output<M>) -> bool {
        self.ct_eq(x).unwrap_u8() == 1
    }
}

impl<M: Mac> Eq for Output<M> {}

#[cfg(feature = "cipher")]
#[cfg_attr(docsrs, doc(cfg(feature = "cipher")))]
/// Trait for MAC functions which can be created from block cipher.
pub trait FromBlockCipher {
    /// Block cipher type
    type Cipher: BlockCipher;

    /// Create new MAC isntance from provided block cipher.
    fn from_cipher(cipher: Self::Cipher) -> Self;
}

#[cfg(feature = "cipher")]
#[cfg_attr(docsrs, doc(cfg(feature = "cipher")))]
impl<T> NewMac for T
where
    T: FromBlockCipher,
    T::Cipher: NewBlockCipher,
{
    type KeySize = <<Self as FromBlockCipher>::Cipher as NewBlockCipher>::KeySize;

    fn new(key: &Key<Self>) -> Self {
        let cipher = <Self as FromBlockCipher>::Cipher::new(key);
        Self::from_cipher(cipher)
    }

    fn new_varkey(key: &[u8]) -> Result<Self, InvalidKeyLength> {
        <Self as FromBlockCipher>::Cipher::new_varkey(key)
            .map_err(|_| InvalidKeyLength)
            .map(Self::from_cipher)
    }
}
