//! A lot of applications and programming language implementations have been
//! recently found to be vulnerable to denial-of-service attacks when a hash
//! function with weak security guarantees, like Murmurhash 3, was used to
//! construct a hash table.
//!
//! In order to address this, Sodium provides the `shorthash()` function.
//! This very fast hash functions outputs short, but unpredictable
//! (without knowing the secret key) values suitable for picking a list in
//! a hash table for a given key.
//!
//! # Selected primitive
//! `shorthash()` is currently an implementation of `SipHash-2-4` as specified in
//! [`SipHash`: a fast short-input PRF](https://131002.net/siphash/)
//!
//! # Example
//! ```
//! use sodiumoxide::crypto::shorthash;
//!
//! let key = shorthash::gen_key();
//! let data_to_hash = b"some data";
//! let digest = shorthash::shorthash(data_to_hash, &key);
//! ```

pub use self::siphash24::*;
pub mod siphash24;
