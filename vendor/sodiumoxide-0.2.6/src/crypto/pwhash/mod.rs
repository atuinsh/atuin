//! Password Hashing
//!
//! Secret keys used to encrypt or sign confidential data have to be chosen from
//! a very large keyspace. However, passwords are usually short, human-generated
//! strings, making dictionary attacks practical.
//!
//! The pwhash operation derives a secret key of any size from a password and a
//! salt.
//!
//! - The generated key has the size defined by the application, no matter what
//!   the password length is.
//! - The same password hashed with same parameters will
//!   always produce the same key.
//! - The same password hashed with different salts
//!   will produce different keys.
//! - The function deriving a key from a password
//!   and a salt is CPU intensive and intentionally requires a fair amount of
//!   memory. Therefore, it mitigates brute-force attacks by requiring a
//!   significant effort to verify each password.
//!
//! Common use cases:
//!
//! - Protecting an on-disk secret key with a password,
//! - Password storage, or rather: storing what it takes to verify a password
//!   without having to store the actual password.
//!
//! # Example (key derivation)
//! ```
//! use sodiumoxide::crypto::secretbox;
//! use sodiumoxide::crypto::pwhash;
//!
//! let passwd = b"Correct Horse Battery Staple";
//! let salt = pwhash::gen_salt();
//! let mut k = secretbox::Key([0; secretbox::KEYBYTES]);
//! {
//!     let secretbox::Key(ref mut kb) = k;
//!     pwhash::derive_key(kb, passwd, &salt,
//!                        pwhash::OPSLIMIT_INTERACTIVE,
//!                        pwhash::MEMLIMIT_INTERACTIVE).unwrap();
//! }
//! ```
//!
//! # Example (password hashing)
//! ```
//! use sodiumoxide::crypto::pwhash;
//! let passwd = b"Correct Horse Battery Staple";
//! let pwh = pwhash::pwhash(passwd,
//!                          pwhash::OPSLIMIT_INTERACTIVE,
//!                          pwhash::MEMLIMIT_INTERACTIVE).unwrap();
//! let pwh_bytes = pwh.as_ref();
//! //store pwh_bytes somewhere
//! ```
//!
//! # Example (password verification)
//! ```
//! use sodiumoxide::crypto::pwhash;
//!
//! let passwd = b"Correct Horse Battery Staple";
//! // in reality we want to load the password hash from somewhere
//! // and we might want to create a `HashedPassword` from it using
//! // `HashedPassword::from_slice(pwhash_bytes).unwrap()`
//! let pwh = pwhash::pwhash(passwd,
//!                          pwhash::OPSLIMIT_INTERACTIVE,
//!                          pwhash::MEMLIMIT_INTERACTIVE).unwrap();
//! assert!(pwhash::pwhash_verify(&pwh, passwd));
//! ```

pub use self::scryptsalsa208sha256::*;
#[macro_use]
mod argon2_macros;
pub mod argon2i13;
pub mod argon2id13;
pub mod scryptsalsa208sha256;
