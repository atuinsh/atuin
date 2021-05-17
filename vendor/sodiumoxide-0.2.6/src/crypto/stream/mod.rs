//! Secret-key encryption
//!
//! # Note
//!
//! Generally speaking [`crypto::secretstream`](../secretstream/index.html) provides a more
//! straightforward API for authenticated encryption with associated data
//! (AEAD) and should be used when this is desired. By contrast, the `stream`
//! module is more appropriate for use cases such as when a variable-length
//! pseudorandom function is needed. [`crypto::secretstream`](../secretstream/index.html)
//! also guarantees messages cannot be truncated, removed, reordered, duplicated, or modified,
//! among other useful guarantees. See the module documentation for more information.
//!
//! # Security Model
//! The `stream()` function, viewed as a function of the nonce for a
//! uniform random key, is designed to meet the standard notion of
//! unpredictability ("PRF"). For a formal definition see, e.g., Section 2.3
//! of Bellare, Kilian, and Rogaway, "The security of the cipher block
//! chaining message authentication code," Journal of Computer and System
//! Sciences 61 (2000), 362–399;
//! <http://www-cse.ucsd.edu/~mihir/papers/cbc.html>.
//!
//! This means that an attacker cannot distinguish this function from a
//! uniform random function. Consequently, if a series of messages is
//! encrypted by `stream_xor()` with a different nonce for each message,
//! the ciphertexts are indistinguishable from uniform random strings of the
//! same length.
//!
//! Note that the length is not hidden. Note also that it is the caller's
//! responsibility to ensure the uniqueness of nonces—for example, by using
//! nonce 1 for the first message, nonce 2 for the second message, etc.
//! Nonces are long enough that randomly generated nonces have negligible
//! risk of collision.
//!
//! `NaCl` does not make any promises regarding the resistance of `stream()` to
//! "related-key attacks." It is the caller's responsibility to use proper
//! key-derivation functions.
//!
//! # Selected primitive
//! `stream()` is `crypto_stream_xsalsa20`, a particular cipher specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html), Section 7.
//! This cipher is conjectured to meet the standard notion of
//! unpredictability.
//!
//! # Alternate primitives
//! NaCl supports the following secret-key encryption functions:
//!
//! --------------------------------------------------------------
//! |`crypto_stream`           |primitive   |KEYBYTES |NONCEBYTES|
//! |--------------------------|------------|---------|----------|
//! |`crypto_stream_chacha20`  |Chacha20/20 |32       |8         |
//! |`crypto_stream_salsa20`   |Salsa20/20  |32       |8         |
//! |`crypto_stream_xsalsa20`  |XSalsa20/20 |32       |24        |
//! |`crypto_stream_xchacha20` |XChacha20/20|32       |24        |
//!
//! Beware that several of these primitives have 8-byte nonces. For those
//! primitives it is no longer true that randomly generated nonces have negligible
//! risk of collision. Callers who are unable to count 1, 2, 3..., and who insist
//! on using these primitives, are advised to use a randomly derived key for each
//! message.
//!
//! # Example (keystream generation)
//! ```
//! use sodiumoxide::crypto::stream;
//!
//! let key = stream::gen_key();
//! let nonce = stream::gen_nonce();
//! let keystream = stream::stream(128, &nonce, &key); // generate 128 bytes of keystream
//! ```
//!
//! # Example (encryption)
//! ```
//! use sodiumoxide::crypto::stream;
//!
//! let key = stream::gen_key();
//! let nonce = stream::gen_nonce();
//! let plaintext = b"some data";
//! let ciphertext = stream::stream_xor(plaintext, &nonce, &key);
//! let their_plaintext = stream::stream_xor(&ciphertext, &nonce, &key);
//! assert_eq!(plaintext, &their_plaintext[..]);
//! ```
//!
//! # Example (in place encryption)
//! ```
//! use sodiumoxide::crypto::stream;
//!
//! let key = stream::gen_key();
//! let nonce = stream::gen_nonce();
//! let plaintext = &mut [0, 1, 2, 3];
//! // encrypt the plaintext
//! stream::stream_xor_inplace(plaintext, &nonce, &key);
//! // decrypt the plaintext
//! stream::stream_xor_inplace(plaintext, &nonce, &key);
//! assert_eq!(plaintext, &mut [0, 1, 2, 3]);
//! ```

pub use self::xsalsa20::*;
#[macro_use]
mod stream_macros;
pub mod chacha20;
pub mod salsa20;
pub mod xchacha20;
pub mod xsalsa20;
