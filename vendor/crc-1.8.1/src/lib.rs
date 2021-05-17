//! # crc
//! Rust implementation of CRC(16, 32, 64)
//!
//! ## Usage
//! ### Compute CRC16
//! ```rust
//! use crc::{crc16, Hasher16};
//!
//! assert_eq!(crc16::checksum_x25(b"123456789"), 0x906e);
//! assert_eq!(crc16::checksum_usb(b"123456789"), 0xb4c8);
//!
//! // use provided or custom polynomial
//! let mut digest = crc16::Digest::new(crc16::X25);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum16(), 0x906e);
//!
//! // with initial
//! let mut digest = crc16::Digest::new_with_initial(crc16::X25, 0u16);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum16(), 0x906e);
//! ```
//!
//! ### Compute CRC32
//! ```rust
//! use crc::{crc32, Hasher32};
//!
//! // CRC-32-IEEE being the most commonly used one
//! assert_eq!(crc32::checksum_ieee(b"123456789"), 0xcbf43926);
//! assert_eq!(crc32::checksum_castagnoli(b"123456789"), 0xe3069283);
//! assert_eq!(crc32::checksum_koopman(b"123456789"), 0x2d3dd0ae);
//!
//! // use provided or custom polynomial
//! let mut digest = crc32::Digest::new(crc32::IEEE);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum32(), 0xcbf43926);
//!
//! // with initial
//! let mut digest = crc32::Digest::new_with_initial(crc32::IEEE, 0u32);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum32(), 0xcbf43926);
//! ```
//!
//! ### Compute CRC64
//! ```rust
//! use crc::{crc64, Hasher64};
//!
//! assert_eq!(crc64::checksum_ecma(b"123456789"), 0x995dc9bbdf1939fa);
//! assert_eq!(crc64::checksum_iso(b"123456789"), 0xb90956c775a41001);
//!
//! // use provided or custom polynomial
//! let mut digest = crc64::Digest::new(crc64::ECMA);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum64(), 0x995dc9bbdf1939fa);
//!
//! // with initial
//! let mut digest = crc64::Digest::new_with_initial(crc64::ECMA, 0u64);
//! digest.write(b"123456789");
//! assert_eq!(digest.sum64(), 0x995dc9bbdf1939fa);
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

pub mod crc16;
pub mod crc32;
pub mod crc64;
mod util;

pub use self::crc16::Hasher16;
pub use self::crc32::Hasher32;
pub use self::crc64::Hasher64;
