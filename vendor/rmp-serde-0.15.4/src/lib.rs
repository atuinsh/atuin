//! This crate connects Rust MessagePack library with [`serde`][serde] providing an ability to
//! easily serialize and deserialize both Rust built-in types, the standard library and custom data
//! structures.
//!
//! ## Motivating example
//!
//! ```
//! let buf = rmp_serde::to_vec(&(42, "the Answer")).unwrap();
//!
//! assert_eq!(
//!     vec![0x92, 0x2a, 0xaa, 0x74, 0x68, 0x65, 0x20, 0x41, 0x6e, 0x73, 0x77, 0x65, 0x72],
//!     buf
//! );
//!
//! assert_eq!((42, "the Answer"), rmp_serde::from_read_ref(&buf).unwrap());
//! ```
//!
//! # Type-based Serialization and Deserialization
//!
//! Serde provides a mechanism for low boilerplate serialization & deserialization of values to and
//! from MessagePack via the serialization API.
//!
//! To be able to serialize a piece of data, it must implement the `serde::Serialize` trait. To be
//! able to deserialize a piece of data, it must implement the `serde::Deserialize` trait. Serde
//! provides an annotation to automatically generate the code for these
//! traits: `#[derive(Serialize, Deserialize)]`.
//!
//! # Examples
//!
//! ```
//! extern crate serde;
//! #[macro_use]
//! extern crate serde_derive;
//! extern crate rmp_serde as rmps;
//!
//! use std::collections::HashMap;
//! use serde::{Deserialize, Serialize};
//! use rmps::{Deserializer, Serializer};
//!
//! #[derive(Debug, PartialEq, Deserialize, Serialize)]
//! struct Human {
//!     age: u32,
//!     name: String,
//! }
//!
//! fn main() {
//!     let mut buf = Vec::new();
//!     let val = Human {
//!         age: 42,
//!         name: "John".into(),
//!     };
//!
//!     val.serialize(&mut Serializer::new(&mut buf)).unwrap();
//! }
//! ```
//!
//! [serde]: https://serde.rs/

#![warn(missing_debug_implementations, missing_docs)]

#[macro_use]
extern crate serde;

use std::fmt::{self, Display, Formatter};
use std::mem;
use std::str::{self, Utf8Error};

use serde::{Deserialize, Serialize};
use serde::de;

pub use crate::decode::{Deserializer, from_read, from_read_ref};
pub use crate::encode::{Serializer, to_vec, to_vec_named};

#[doc(hidden)]
pub use crate::decode::from_slice;

pub mod config;
pub mod decode;
pub mod encode;

/// Name of Serde newtype struct to Represent Msgpack's Ext
/// Msgpack Ext: Ext(tag, binary)
/// Serde data model: _ExtStruct((tag, binary))
/// Example Serde impl for custom type:
///
/// ```ignore
/// #[derive(Debug, PartialEq, Serialize, Deserialize)]
/// #[serde(rename = "_ExtStruct")]
/// struct ExtStruct((i8, serde_bytes::ByteBuf));
///
/// test_round(ExtStruct((2, serde_bytes::ByteBuf::from(vec![5]))),
///            Value::Ext(2, vec![5]));
/// ```
pub const MSGPACK_EXT_STRUCT_NAME: &str = "_ExtStruct";

/// Helper that allows both to encode and decode strings no matter whether they contain valid or
/// invalid UTF-8.
///
/// Regardless of validity the UTF-8 content this type will always be serialized as a string.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    s: Result<String, (Vec<u8>, Utf8Error)>,
}

impl Raw {
    /// Constructs a new `Raw` from the UTF-8 string.
    #[inline]
    pub fn new(v: String) -> Self {
        Self { s: Ok(v) }
    }

    /// Converts a vector of bytes to a `Raw`.
    pub fn from_utf8(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(v) => Raw::new(v),
            Err(err) => {
                let e = err.utf8_error();
                Self {
                    s: Err((err.into_bytes(), e))
                }
            }
        }
    }

    /// Returns `true` if the raw is valid UTF-8.
    #[inline]
    pub fn is_str(&self) -> bool {
        self.s.is_ok()
    }

    /// Returns `true` if the raw contains invalid UTF-8 sequence.
    #[inline]
    pub fn is_err(&self) -> bool {
        self.s.is_err()
    }

    /// Returns the string reference if the raw is valid UTF-8, or else `None`.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self.s {
            Ok(ref s) => Some(s.as_str()),
            Err(..) => None,
        }
    }

    /// Returns the underlying `Utf8Error` if the raw contains invalid UTF-8 sequence, or
    /// else `None`.
    #[inline]
    pub fn as_err(&self) -> Option<&Utf8Error> {
        match self.s {
            Ok(..) => None,
            Err((_, ref err)) => Some(&err),
        }
    }

    /// Returns a byte slice of this raw's contents.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        match self.s {
            Ok(ref s) => s.as_bytes(),
            Err(ref err) => &err.0[..],
        }
    }

    /// Consumes this object, yielding the string if the raw is valid UTF-8, or else `None`.
    #[inline]
    pub fn into_str(self) -> Option<String> {
        self.s.ok()
    }

    /// Converts a `Raw` into a byte vector.
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        match self.s {
            Ok(s) => s.into_bytes(),
            Err(err) => err.0,
        }
    }
}

impl Serialize for Raw {
    fn serialize<S>(&self, se: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer
    {
        let s = match self.s {
            Ok(ref s) => s.as_str(),
            Err((ref b, ..)) => unsafe { mem::transmute(&b[..]) },
        };

        se.serialize_str(s)
    }
}

struct RawVisitor;

impl<'de> de::Visitor<'de> for RawVisitor {
    type Value = Raw;

    #[cold]
    fn expecting(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        "string or bytes".fmt(fmt)
    }

    #[inline]
    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(Raw { s: Ok(v) })
    }

    #[inline]
    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where E: de::Error
    {
        Ok(Raw { s: Ok(v.into()) })
    }

    #[inline]
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where E: de::Error
    {
        let s = match str::from_utf8(v) {
            Ok(s) => Ok(s.into()),
            Err(err) => Err((v.into(), err)),
        };

        Ok(Raw { s })
    }

    #[inline]
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where E: de::Error
    {
        let s = match String::from_utf8(v) {
            Ok(s) => Ok(s),
            Err(err) => {
                let e = err.utf8_error();
                Err((err.into_bytes(), e))
            }
        };

        Ok(Raw { s })
    }
}

impl<'de> Deserialize<'de> for Raw {
    #[inline]
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
        where D: de::Deserializer<'de>
    {
        de.deserialize_any(RawVisitor)
    }
}

/// Helper that allows both to encode and decode strings no matter whether they contain valid or
/// invalid UTF-8.
///
/// Regardless of validity the UTF-8 content this type will always be serialized as a string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawRef<'a> {
    s: Result<&'a str, (&'a [u8], Utf8Error)>,
}

impl<'a> RawRef<'a> {
    /// Constructs a new `RawRef` from the UTF-8 string.
    #[inline]
    pub fn new(v: &'a str) -> Self {
        Self { s: Ok(v) }
    }

    /// Converts a vector of bytes to a `RawRef`.
    pub fn from_utf8(v: &'a [u8]) -> Self {
        match str::from_utf8(v) {
            Ok(v) => RawRef::new(v),
            Err(err) => {
                Self {
                    s: Err((v, err))
                }
            }
        }
    }

    /// Returns `true` if the raw is valid UTF-8.
    #[inline]
    pub fn is_str(&self) -> bool {
        self.s.is_ok()
    }

    /// Returns `true` if the raw contains invalid UTF-8 sequence.
    #[inline]
    pub fn is_err(&self) -> bool {
        self.s.is_err()
    }

    /// Returns the string reference if the raw is valid UTF-8, or else `None`.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self.s {
            Ok(ref s) => Some(s),
            Err(..) => None,
        }
    }

    /// Returns the underlying `Utf8Error` if the raw contains invalid UTF-8 sequence, or
    /// else `None`.
    #[inline]
    pub fn as_err(&self) -> Option<&Utf8Error> {
        match self.s {
            Ok(..) => None,
            Err((_, ref err)) => Some(&err),
        }
    }

    /// Returns a byte slice of this raw's contents.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        match self.s {
            Ok(ref s) => s.as_bytes(),
            Err(ref err) => &err.0[..],
        }
    }
}

impl<'a> Serialize for RawRef<'a> {
    fn serialize<S>(&self, se: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer
    {
        let s = match self.s {
            Ok(ref s) => s,
            Err((ref b, ..)) => unsafe { mem::transmute(b) },
        };

        se.serialize_str(s)
    }
}

struct RawRefVisitor;

impl<'de> de::Visitor<'de> for RawRefVisitor {
    type Value = RawRef<'de>;

    #[cold]
    fn expecting(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        "string or bytes".fmt(fmt)
    }

    #[inline]
    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
        where E: de::Error
    {
        Ok(RawRef { s: Ok(v) })
    }

    #[inline]
    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
        where E: de::Error
    {
        let s = match str::from_utf8(v) {
            Ok(s) => Ok(s),
            Err(err) => Err((v, err)),
        };

        Ok(RawRef { s })
    }
}

impl<'de> Deserialize<'de> for RawRef<'de> {
    #[inline]
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
        where D: de::Deserializer<'de>
    {
        de.deserialize_any(RawRefVisitor)
    }
}
