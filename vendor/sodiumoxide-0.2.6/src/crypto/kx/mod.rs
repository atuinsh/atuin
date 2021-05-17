//! Key exchange
//!
//! Using the key exchange API, two parties can securely compute a set of shared keys using their
//! peer's public key and their own secret key.
//!
//! This API was introduced in libsodium 1.0.12.
//!
//! # Example
//!
//! ```
//! use sodiumoxide::crypto::kx;
//!
//! // client-side
//! let (client_pk, client_sk) = kx::gen_keypair();
//!
//! // server-side
//! let (server_pk, server_sk) = kx::gen_keypair();
//!
//! // client and server exchanges client_pk and server_pk
//!
//! // client deduces the two session keys rx1 and tx1
//! let (rx1, tx1) = match kx::client_session_keys(&client_pk, &client_sk, &server_pk) {
//!     Ok((rx, tx)) => (rx, tx),
//!     Err(()) => panic!("bad server signature"),
//! };
//!
//! // server performs the same operation
//! let (rx2, tx2) = match kx::server_session_keys(&server_pk, &server_sk, &client_pk) {
//!     Ok((rx, tx)) => (rx, tx),
//!     Err(()) => panic!("bad client signature"),
//! };
//!
//! assert!(rx1==tx2);
//! assert!(rx2==tx1);
//!
//! ```

pub use self::x25519blake2b::*;
pub mod x25519blake2b;
