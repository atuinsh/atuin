//! Secret-key authentication
//!
//! # Security model
//! The `authenticate()` function, viewed as a function of the
//! message for a uniform random key, is designed to meet the standard
//! notion of unforgeability. This means that an attacker cannot find
//! authenticators for any messages not authenticated by the sender, even if
//! the attacker has adaptively influenced the messages authenticated by the
//! sender. For a formal definition see, e.g., Section 2.4 of Bellare,
//! Kilian, and Rogaway, "The security of the cipher block chaining message
//! authentication code," Journal of Computer and System Sciences 61 (2000),
//! 362–399; <http://www-cse.ucsd.edu/~mihir/papers/cbc.html>.
//!
//! `NaCl` does not make any promises regarding "strong" unforgeability;
//! perhaps one valid authenticator can be converted into another valid
//! authenticator for the same message. `NaCl` also does not make any promises
//! regarding "truncated unforgeability."
//!
//! # Selected primitive
//! `authenticate()` is currently an implementation of
//! `HMAC-SHA-512-256`, i.e., the first 256 bits of `HMAC-SHA-512`.
//! `HMAC-SHA-512-256` is conjectured to meet the standard notion of
//! unforgeability.
//!
//! # Alternate primitives
//! `NaCl` supports the following secret-key authentication functions:
//!
//! -----------------------------------------------------------------
//! |`crypto_auth`               |primitive          |BYTES|KEYBYTES|
//! |----------------------------|-------------------|-----|--------|
//! |`crypto_auth_hmacsha256`    |`HMAC_SHA-256`     |32   |32      |
//! |`crypto_auth_hmacsha512256` |`HMAC_SHA-512-256` |32   |32      |
//! |`crypto_auth_hmacsha512`    |`HMAC_SHA-512`     |64   |32      |
//!
//! # Example (simple interface)
//! ```
//! use sodiumoxide::crypto::auth;
//!
//! let key = auth::gen_key();
//! let data_to_authenticate = b"some data";
//! let tag = auth::authenticate(data_to_authenticate, &key);
//! assert!(auth::verify(&tag, data_to_authenticate, &key));
//! ```
//!
//! # Example (streaming interface)
//! ```
//! use sodiumoxide::crypto::auth;
//! use sodiumoxide::randombytes;
//!
//! let key = randombytes::randombytes(123);
//!
//! let data_part_1 = b"some data";
//! let data_part_2 = b"some other data";
//! let mut state = auth::State::init(&key);
//! state.update(data_part_1);
//! state.update(data_part_2);
//! let tag1 = state.finalize();
//!
//! let data_2_part_1 = b"some datasome ";
//! let data_2_part_2 = b"other data";
//! let mut state = auth::State::init(&key);
//! state.update(data_2_part_1);
//! state.update(data_2_part_2);
//! let tag2 = state.finalize();
//! assert_eq!(tag1, tag2);
//! ```

pub use self::hmacsha512256::*;
#[macro_use]
mod auth_macros;
#[macro_use]
mod auth_state_macros;
pub mod hmacsha256;
pub mod hmacsha512;
pub mod hmacsha512256;
