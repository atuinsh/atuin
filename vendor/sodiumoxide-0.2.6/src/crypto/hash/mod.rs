//! Hashing
//!
//! # Security model
//! The `hash()` function is designed to be usable as a strong
//! component of DSA, RSA-PSS, key derivation, hash-based
//! message-authentication codes, hash-based ciphers, and various other
//! common applications.  "Strong" means that the security of these
//! applications, when instantiated with `hash()`, is the same
//! as the security of the applications against generic attacks. In
//! particular, the `hash()` function is designed to make
//! finding collisions difficult.
//!
//! # Selected primitive
//! `hash()` is currently an implementation of `SHA-512`.
//!
//! There has been considerable degradation of public confidence in the
//! security conjectures for many hash functions, including `SHA-512`.
//! However, for the moment, there do not appear to be alternatives that
//! inspire satisfactory levels of confidence. One can hope that NIST's
//! SHA-3 competition will improve the situation.
//!
//! # Alternate primitives
//! `NaCl` supports the following hash functions:
//!
//! -----------------------------------------
//! |`crypto_hash`        |primitive |BYTES |
//! |---------------------|----------|------|
//! |`crypto_hash_sha256` |`SHA-256` |32    |
//! |`crypto_hash_sha512` |`SHA-512` |64    |
//!
//! # Example
//! ```
//! use sodiumoxide::crypto::hash;
//!
//! let data_to_hash = b"some data";
//! let digest = hash::hash(data_to_hash);
//!
//! let mut hash_state = hash::State::new();
//! hash_state.update(b"some ");
//! hash_state.update(b"data!");
//! let digest = hash_state.finalize();
//! ```

pub use self::sha512::*;
#[macro_use]
mod hash_macros;
pub mod sha256;
pub mod sha512;
