//! Stream encryption/file encryption
//!
//! This high-level API encrypts a sequence of messages, or a single message split into an arbitrary
//! number of chunks, using a secret key, with the following properties:
//!
//! * Messages cannot be truncated, removed, reordered, duplicated or modified without this being
//!   detected by the decryption functions.
//! * The same sequence encrypted twice will produce different ciphertexts.
//! * An authentication tag is added to each encrypted message: stream corruption will be detected
//!   early, without having to read the stream until the end.
//! * Each message can include additional data (ex: timestamp, protocol version) in the computation
//!   of the authentication tag.
//! * Messages can have different sizes.
//! * There are no practical limits to the total length of the stream,
//!   or to the total number of individual messages.
//! * Ratcheting: at any point in the stream, it is possible to "forget" the key used to encrypt
//!   the previous messages, and switch to a new key.
//!
//! This API can be used to securely send an ordered sequence of messages to a peer.
//! Since the length of the stream is not limited, it can also be used to encrypt files
//! regardless of their size.
//!
//! It transparently generates nonces and automatically handles key rotation.
//!
//! The `crypto_secretstream_*()` API was introduced in libsodium 1.0.14.
//!
//! # Example
//! ```
//! use sodiumoxide::crypto::secretstream::{gen_key, Stream, Tag};
//!
//! let msg1 = "some message 1";
//! let msg2 = "other message";
//! let msg3 = "final message";
//!
//! // initialize encrypt secret stream
//! let key = gen_key();
//! let (mut enc_stream, header) = Stream::init_push(&key).unwrap();
//!
//! // encrypt first message, tagging it as message.
//! let ciphertext1 = enc_stream.push(msg1.as_bytes(), None, Tag::Message).unwrap();
//!
//! // encrypt second message, tagging it as push.
//! let ciphertext2 = enc_stream.push(msg2.as_bytes(), None, Tag::Push).unwrap();
//!
//! // encrypt third message, tagging it as final.
//! let ciphertext3 = enc_stream.push(msg3.as_bytes(), None, Tag::Final).unwrap();
//!
//! // initialize decrypt secret stream
//! let mut dec_stream = Stream::init_pull(&header, &key).unwrap();
//!
//! // decrypt first message.
//! assert!(!dec_stream.is_finalized());
//! let (decrypted1, tag1) = dec_stream.pull(&ciphertext1, None).unwrap();
//! assert_eq!(tag1, Tag::Message);
//! assert_eq!(msg1.as_bytes(), &decrypted1[..]);
//!
//! // decrypt second message.
//! assert!(!dec_stream.is_finalized());
//! let (decrypted2, tag2) = dec_stream.pull(&ciphertext2, None).unwrap();
//! assert_eq!(tag2, Tag::Push);
//! assert_eq!(msg2.as_bytes(), &decrypted2[..]);
//!
//! // decrypt last message.
//! assert!(!dec_stream.is_finalized());
//! let (decrypted3, tag3) = dec_stream.pull(&ciphertext3, None).unwrap();
//! assert_eq!(tag3, Tag::Final);
//! assert_eq!(msg3.as_bytes(), &decrypted3[..]);
//!
//! // dec_stream is now finalized.
//! assert!(dec_stream.is_finalized());
//!
//! ```
pub use self::xchacha20poly1305::*;
#[macro_use]
mod secretstream_macros;
pub mod xchacha20poly1305;
