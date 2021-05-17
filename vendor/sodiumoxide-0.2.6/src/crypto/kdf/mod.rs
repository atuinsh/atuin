//! Key derivation
//!
//! Multiple secret subkeys can be derived from a single master key. Given the master key and a key
//! identifier, a subkey can be deterministically computed. However, given a subkey, an attacker
//! cannot compute the master key nor any other subkeys.
//!
//! This API was introduced in libsodium 1.0.12
//!
//! # Example
//!
//! ```
//! use sodiumoxide::crypto::kdf;
//! use sodiumoxide::crypto::secretbox;
//!
//! const CONTEXT: [u8; 8] = *b"Examples";
//!
//! let key = kdf::gen_key();
//!
//! let mut key1 = secretbox::Key([0; secretbox::KEYBYTES]);
//! kdf::derive_from_key(&mut key1.0[..], 1, CONTEXT, &key).unwrap();
//!
//! let mut key2 = secretbox::Key([0; secretbox::KEYBYTES]);
//! kdf::derive_from_key(&mut key2.0[..], 2, CONTEXT, &key).unwrap();
//!
//! let mut key3 = secretbox::Key([0; secretbox::KEYBYTES]);
//! kdf::derive_from_key(&mut key3.0[..], 3, CONTEXT, &key).unwrap();
//! ```

pub mod blake2b;
pub use self::blake2b::*;
