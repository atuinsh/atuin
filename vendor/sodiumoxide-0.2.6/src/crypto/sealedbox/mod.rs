//! Sealed Boxes
//!
//! # Purpose
//! Sealed boxes are designed to anonymously send messages to a recipient given
//! its public key.
//!
//! Only the recipient can decrypt these messages, using its private key. While
//! the recipient can verify the integrity of the message, it cannot verify the
//! identity of the sender.
//!
//! A message is encrypted using an ephemeral key pair, whose secret part is
//! destroyed right after the encryption process.
//!
//! Without knowing the secret key used for a given message, the sender cannot
//! decrypt its own message later. And without additional data, a message
//! cannot be correlated with the identity of its sender.
//!
//! # Algorithm Details
//! Sealed boxes leverage the `crypto_box` construction (X25519, XSalsa20-Poly1305).
//!
//! The format of a sealed box is
//!
//! ```c
//! ephemeral_pk || box(m, recipient_pk, ephemeral_sk,
//!                     nonce=blake2b(ephemeral_pk, recipient_pk))
//! ```
//!

pub use self::curve25519blake2bxsalsa20poly1305::*;
pub mod curve25519blake2bxsalsa20poly1305;
