//! Public-key authenticated encryption
//!
//! # Security model
//! The `seal()` function is designed to meet the standard notions of privacy and
//! third-party unforgeability for a public-key authenticated-encryption scheme
//! using nonces. For formal definitions see, e.g., Jee Hea An, "Authenticated
//! encryption in the public-key setting: security notions and analyses,"
//! <http://eprint.iacr.org/2001/079>.
//!
//! Distinct messages between the same {sender, receiver} set are required
//! to have distinct nonces. For example, the lexicographically smaller
//! public key can use nonce 1 for its first message to the other key, nonce
//! 3 for its second message, nonce 5 for its third message, etc., while the
//! lexicographically larger public key uses nonce 2 for its first message
//! to the other key, nonce 4 for its second message, nonce 6 for its third
//! message, etc. Nonces are long enough that randomly generated nonces have
//! negligible risk of collision.
//!
//! There is no harm in having the same nonce for different messages if the
//! {sender, receiver} sets are different. This is true even if the sets
//! overlap. For example, a sender can use the same nonce for two different
//! messages if the messages are sent to two different public keys.
//!
//! The `seal()` function is not meant to provide non-repudiation. On the
//! contrary: the `seal()` function guarantees repudiability. A receiver
//! can freely modify a boxed message, and therefore cannot convince third
//! parties that this particular message came from the sender. The sender
//! and receiver are nevertheless protected against forgeries by other
//! parties. In the terminology of
//! <http://groups.google.com/group/sci.crypt/msg/ec5c18b23b11d82c>,
//! crypto_box uses "public-key authenticators" rather than "public-key
//! signatures."
//!
//! Users who want public verifiability (or receiver-assisted public
//! verifiability) should instead use signatures (or signcryption).
//! Signature support is a high priority for `NaCl`; a signature API will be
//! described in subsequent `NaCl` documentation.
//!
//! # Selected primitive
//! `seal()` is `crypto_box_curve25519xsalsa20poly1305` , a particular
//! combination of Curve25519, Salsa20, and Poly1305 specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html).
//!
//! This function is conjectured to meet the standard notions of privacy and
//! third-party unforgeability.
//!
//! # Example (simple interface)
//! ```
//! use sodiumoxide::crypto::box_;
//!
//! let (ourpk, oursk) = box_::gen_keypair();
//! // normally theirpk is sent by the other party
//! let (theirpk, theirsk) = box_::gen_keypair();
//! let nonce = box_::gen_nonce();
//! let plaintext = b"some data";
//! let ciphertext = box_::seal(plaintext, &nonce, &theirpk, &oursk);
//! let their_plaintext = box_::open(&ciphertext, &nonce, &ourpk, &theirsk).unwrap();
//! assert!(plaintext == &their_plaintext[..]);
//! ```
//! # Example (precomputation interface)
//! ```
//! use sodiumoxide::crypto::box_;
//!
//! let (ourpk, oursk) = box_::gen_keypair();
//! let (theirpk, theirsk) = box_::gen_keypair();
//! let our_precomputed_key = box_::precompute(&theirpk, &oursk);
//! let nonce = box_::gen_nonce();
//! let plaintext = b"plaintext";
//! let ciphertext = box_::seal_precomputed(plaintext, &nonce, &our_precomputed_key);
//! // this will be identical to our_precomputed_key
//! let their_precomputed_key = box_::precompute(&ourpk, &theirsk);
//! let their_plaintext = box_::open_precomputed(&ciphertext, &nonce,
//!                                              &their_precomputed_key).unwrap();
//! assert!(plaintext == &their_plaintext[..]);
//! ```

pub use self::curve25519xsalsa20poly1305::*;
pub mod curve25519xsalsa20poly1305;
