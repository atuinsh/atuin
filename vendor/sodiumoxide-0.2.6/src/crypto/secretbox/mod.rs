//! Secret-key authenticated encryption
//!
//! # Security model
//! The `seal()` function is designed to meet the standard notions of privacy and
//! authenticity for a secret-key authenticated-encryption scheme using nonces. For
//! formal definitions see, e.g., Bellare and Namprempre, "Authenticated
//! encryption: relations among notions and analysis of the generic composition
//! paradigm," Lecture Notes in Computer Science 1976 (2000), 531–545,
//! <http://www-cse.ucsd.edu/~mihir/papers/oem.html>.
//!
//! Note that the length is not hidden. Note also that it is the caller's
//! responsibility to ensure the uniqueness of nonces—for example, by using
//! nonce 1 for the first message, nonce 2 for the second message, etc.
//! Nonces are long enough that randomly generated nonces have negligible
//! risk of collision.
//!
//! # Selected primitive
//! `seal()` is `crypto_secretbox_xsalsa20poly1305`, a particular
//! combination of Salsa20 and Poly1305 specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html).
//!
//! This function is conjectured to meet the standard notions of privacy and
//! authenticity.
//!
//! # Example
//! ```
//! use sodiumoxide::crypto::secretbox;
//! let key = secretbox::gen_key();
//! let nonce = secretbox::gen_nonce();
//! let plaintext = b"some data";
//! let ciphertext = secretbox::seal(plaintext, &nonce, &key);
//! let their_plaintext = secretbox::open(&ciphertext, &nonce, &key).unwrap();
//! assert!(plaintext == &their_plaintext[..]);
//! ```

pub use self::xsalsa20poly1305::*;
pub mod xsalsa20poly1305;
