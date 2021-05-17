//! Public-key signatures
//!
//! # Security model
//! The [`sign()`](ed25519/fn.sign.html) function is designed to meet the standard
//! notion of unforgeability for a public-key signature scheme under
//! chosen-message attacks.
//!
//! # Selected primitive
//! [`crypto::sign::sign`](ed25519/fn.sign.html) is `ed25519`, a signature scheme specified in
//! [Ed25519](http://ed25519.cr.yp.to/). This function is conjectured to meet the
//! standard notion of unforgeability for a public-key signature scheme under
//! chosen-message attacks.
//!
//! # Alternate primitives
//!
//! ----------------------------------------------------------------------------------
//! |`crypto_sign`                         | PUBLICKEYBYTES | SECRETKEYBYTES | BYTES |
//! |--------------------------------------|----------------|----------------|-------|
//! |`crypto_sign_ed25519`                 | 32             | 64             | 64    |
//! |`crypto_sign_edwards25519sha512batch` | 32             | 64             | 64    |
//!
//! `crypto_sign_edwards25519sha512batch` is a prototype. It has been replaced with
//! Ed25519 and is only kept here for compatibility reasons.
//!
//! # Example
//! ```
//! use sodiumoxide::crypto::sign;
//! let (pk, sk) = sign::gen_keypair();
//! let data_to_sign = b"some data";
//! let signed_data = sign::sign(data_to_sign, &sk);
//! let verified_data = sign::verify(&signed_data, &pk).unwrap();
//! assert!(data_to_sign == &verified_data[..]);
//! ```
//!
//! # Example (detached signatures)
//! ```
//! use sodiumoxide::crypto::sign;
//! let (pk, sk) = sign::gen_keypair();
//! let data_to_sign = b"some data";
//! let signature = sign::sign_detached(data_to_sign, &sk);
//! assert!(sign::verify_detached(&signature, data_to_sign, &pk));
//! ```
pub use self::ed25519::*;
pub mod ed25519;
