// Copyright 2013-2014 The Rust Project Developers.
// Copyright 2018 The Uuid Project Developers.
//
// See the COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A Builder type for [`Uuid`]s.
//!
//! [`Uuid`]: ../struct.Uuid.html

mod error;
pub(crate) use self::error::Error;

use crate::prelude::*;

impl Uuid {
    /// The 'nil UUID'.
    ///
    /// The nil UUID is special form of UUID that is specified to have all
    /// 128 bits set to zero, as defined in [IETF RFC 4122 Section 4.1.7][RFC].
    ///
    /// [RFC]: https://tools.ietf.org/html/rfc4122.html#section-4.1.7
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use uuid::Uuid;
    ///
    /// let uuid = Uuid::nil();
    ///
    /// assert_eq!(
    ///     uuid.to_hyphenated().to_string(),
    ///     "00000000-0000-0000-0000-000000000000"
    /// );
    /// ```
    pub const fn nil() -> Self {
        Uuid::from_bytes([0; 16])
    }

    /// Creates a UUID from four field values in big-endian order.
    ///
    /// # Errors
    ///
    /// This function will return an error if `d4`'s length is not 8 bytes.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use uuid::Uuid;
    ///
    /// let d4 = [12, 3, 9, 56, 54, 43, 8, 9];
    ///
    /// let uuid = Uuid::from_fields(42, 12, 5, &d4);
    /// let uuid = uuid.map(|uuid| uuid.to_hyphenated().to_string());
    ///
    /// let expected_uuid =
    ///     Ok(String::from("0000002a-000c-0005-0c03-0938362b0809"));
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    pub fn from_fields(
        d1: u32,
        d2: u16,
        d3: u16,
        d4: &[u8],
    ) -> Result<Uuid, crate::Error> {
        const D4_LEN: usize = 8;

        let len = d4.len();

        if len != D4_LEN {
            Err(Error::new(D4_LEN, len))?;
        }

        Ok(Uuid::from_bytes([
            (d1 >> 24) as u8,
            (d1 >> 16) as u8,
            (d1 >> 8) as u8,
            d1 as u8,
            (d2 >> 8) as u8,
            d2 as u8,
            (d3 >> 8) as u8,
            d3 as u8,
            d4[0],
            d4[1],
            d4[2],
            d4[3],
            d4[4],
            d4[5],
            d4[6],
            d4[7],
        ]))
    }

    /// Creates a UUID from four field values in little-endian order.
    ///
    /// The bytes in the `d1`, `d2` and `d3` fields will
    /// be converted into big-endian order.
    ///
    /// # Examples
    ///
    /// ```
    /// use uuid::Uuid;
    ///
    /// let d1 = 0xAB3F1097u32;
    /// let d2 = 0x501Eu16;
    /// let d3 = 0xB736u16;
    /// let d4 = [12, 3, 9, 56, 54, 43, 8, 9];
    ///
    /// let uuid = Uuid::from_fields_le(d1, d2, d3, &d4);
    /// let uuid = uuid.map(|uuid| uuid.to_hyphenated().to_string());
    ///
    /// let expected_uuid =
    ///     Ok(String::from("97103fab-1e50-36b7-0c03-0938362b0809"));
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    pub fn from_fields_le(
        d1: u32,
        d2: u16,
        d3: u16,
        d4: &[u8],
    ) -> Result<Uuid, crate::Error> {
        const D4_LEN: usize = 8;

        let len = d4.len();

        if len != D4_LEN {
            Err(Error::new(D4_LEN, len))?;
        }

        Ok(Uuid::from_bytes([
            d1 as u8,
            (d1 >> 8) as u8,
            (d1 >> 16) as u8,
            (d1 >> 24) as u8,
            (d2) as u8,
            (d2 >> 8) as u8,
            d3 as u8,
            (d3 >> 8) as u8,
            d4[0],
            d4[1],
            d4[2],
            d4[3],
            d4[4],
            d4[5],
            d4[6],
            d4[7],
        ]))
    }

    /// Creates a UUID from a 128bit value in big-endian order.
    pub const fn from_u128(v: u128) -> Self {
        Uuid::from_bytes([
            (v >> 120) as u8,
            (v >> 112) as u8,
            (v >> 104) as u8,
            (v >> 96) as u8,
            (v >> 88) as u8,
            (v >> 80) as u8,
            (v >> 72) as u8,
            (v >> 64) as u8,
            (v >> 56) as u8,
            (v >> 48) as u8,
            (v >> 40) as u8,
            (v >> 32) as u8,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ])
    }

    /// Creates a UUID from a 128bit value in little-endian order.
    pub const fn from_u128_le(v: u128) -> Self {
        Uuid::from_bytes([
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
            (v >> 24) as u8,
            (v >> 32) as u8,
            (v >> 40) as u8,
            (v >> 48) as u8,
            (v >> 56) as u8,
            (v >> 64) as u8,
            (v >> 72) as u8,
            (v >> 80) as u8,
            (v >> 88) as u8,
            (v >> 96) as u8,
            (v >> 104) as u8,
            (v >> 112) as u8,
            (v >> 120) as u8,
        ])
    }

    /// Creates a UUID using the supplied big-endian bytes.
    ///
    /// # Errors
    ///
    /// This function will return an error if `b` has any length other than 16.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use uuid::Uuid;
    ///
    /// let bytes = [4, 54, 67, 12, 43, 2, 98, 76, 32, 50, 87, 5, 1, 33, 43, 87];
    ///
    /// let uuid = Uuid::from_slice(&bytes);
    /// let uuid = uuid.map(|uuid| uuid.to_hyphenated().to_string());
    ///
    /// let expected_uuid =
    ///     Ok(String::from("0436430c-2b02-624c-2032-570501212b57"));
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    ///
    /// An incorrect number of bytes:
    ///
    /// ```
    /// use uuid::Uuid;
    ///
    /// let bytes = [4, 54, 67, 12, 43, 2, 98, 76];
    ///
    /// let uuid = Uuid::from_slice(&bytes);
    ///
    /// assert!(uuid.is_err());
    /// ```
    pub fn from_slice(b: &[u8]) -> Result<Uuid, crate::Error> {
        const BYTES_LEN: usize = 16;

        let len = b.len();

        if len != BYTES_LEN {
            Err(Error::new(BYTES_LEN, len))?;
        }

        let mut bytes: Bytes = [0; 16];
        bytes.copy_from_slice(b);
        Ok(Uuid::from_bytes(bytes))
    }

    /// Creates a UUID using the supplied big-endian bytes.
    pub const fn from_bytes(bytes: Bytes) -> Uuid {
        Uuid(bytes)
    }
}

/// A builder struct for creating a UUID.
///
/// # Examples
///
/// Creating a v4 UUID from externally generated bytes:
///
/// ```
/// use uuid::{Builder, Variant, Version};
///
/// # let rng = || [
/// #     70, 235, 208, 238, 14, 109, 67, 201, 185, 13, 204, 195, 90,
/// # 145, 63, 62,
/// # ];
/// let random_bytes = rng();
/// let uuid = Builder::from_bytes(random_bytes)
///     .set_variant(Variant::RFC4122)
///     .set_version(Version::Random)
///     .build();
/// ```
// TODO: remove in 1.0.0
#[allow(dead_code)]
#[deprecated]
pub type Builder = crate::Builder;

impl crate::Builder {
    /// Creates a `Builder` using the supplied big-endian bytes.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// let bytes: uuid::Bytes = [
    ///     70, 235, 208, 238, 14, 109, 67, 201, 185, 13, 204, 195, 90, 145, 63, 62,
    /// ];
    ///
    /// let mut builder = uuid::Builder::from_bytes(bytes);
    /// let uuid = builder.build().to_hyphenated().to_string();
    ///
    /// let expected_uuid = String::from("46ebd0ee-0e6d-43c9-b90d-ccc35a913f3e");
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    ///
    /// An incorrect number of bytes:
    ///
    /// ```compile_fail
    /// let bytes: uuid::Bytes = [4, 54, 67, 12, 43, 2, 98, 76]; // doesn't compile
    ///
    /// let uuid = uuid::Builder::from_bytes(bytes);
    /// ```
    pub const fn from_bytes(b: Bytes) -> Self {
        Builder(b)
    }

    /// Creates a `Builder` using the supplied big-endian bytes.
    ///
    /// # Errors
    ///
    /// This function will return an error if `b` has any length other than 16.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// let bytes = [4, 54, 67, 12, 43, 2, 98, 76, 32, 50, 87, 5, 1, 33, 43, 87];
    ///
    /// let builder = uuid::Builder::from_slice(&bytes);
    /// let uuid =
    ///     builder.map(|mut builder| builder.build().to_hyphenated().to_string());
    ///
    /// let expected_uuid =
    ///     Ok(String::from("0436430c-2b02-624c-2032-570501212b57"));
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    ///
    /// An incorrect number of bytes:
    ///
    /// ```
    /// let bytes = [4, 54, 67, 12, 43, 2, 98, 76];
    ///
    /// let builder = uuid::Builder::from_slice(&bytes);
    ///
    /// assert!(builder.is_err());
    /// ```
    pub fn from_slice(b: &[u8]) -> Result<Self, crate::Error> {
        const BYTES_LEN: usize = 16;

        let len = b.len();

        if len != BYTES_LEN {
            Err(Error::new(BYTES_LEN, len))?;
        }

        let mut bytes: crate::Bytes = [0; 16];
        bytes.copy_from_slice(b);
        Ok(Self::from_bytes(bytes))
    }

    /// Creates a `Builder` from four big-endian field values.
    ///
    /// # Errors
    ///
    /// This function will return an error if `d4`'s length is not 8 bytes.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// let d4 = [12, 3, 9, 56, 54, 43, 8, 9];
    ///
    /// let builder = uuid::Builder::from_fields(42, 12, 5, &d4);
    /// let uuid =
    ///     builder.map(|mut builder| builder.build().to_hyphenated().to_string());
    ///
    /// let expected_uuid =
    ///     Ok(String::from("0000002a-000c-0005-0c03-0938362b0809"));
    ///
    /// assert_eq!(expected_uuid, uuid);
    /// ```
    ///
    /// An invalid length:
    ///
    /// ```
    /// let d4 = [12];
    ///
    /// let builder = uuid::Builder::from_fields(42, 12, 5, &d4);
    ///
    /// assert!(builder.is_err());
    /// ```
    pub fn from_fields(
        d1: u32,
        d2: u16,
        d3: u16,
        d4: &[u8],
    ) -> Result<Self, crate::Error> {
        Uuid::from_fields(d1, d2, d3, d4).map(|uuid| {
            let bytes = *uuid.as_bytes();

            crate::Builder::from_bytes(bytes)
        })
    }

    /// Creates a `Builder` from a big-endian 128bit value.
    pub fn from_u128(v: u128) -> Self {
        crate::Builder::from_bytes(*Uuid::from_u128(v).as_bytes())
    }

    /// Creates a `Builder` with an initial [`Uuid::nil`].
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use uuid::Builder;
    ///
    /// let mut builder = Builder::nil();
    ///
    /// assert_eq!(
    ///     builder.build().to_hyphenated().to_string(),
    ///     "00000000-0000-0000-0000-000000000000"
    /// );
    /// ```
    pub const fn nil() -> Self {
        Builder([0; 16])
    }

    /// Specifies the variant of the UUID.
    pub fn set_variant(&mut self, v: crate::Variant) -> &mut Self {
        let byte = self.0[8];

        self.0[8] = match v {
            crate::Variant::NCS => byte & 0x7f,
            crate::Variant::RFC4122 => (byte & 0x3f) | 0x80,
            crate::Variant::Microsoft => (byte & 0x1f) | 0xc0,
            crate::Variant::Future => (byte & 0x1f) | 0xe0,
        };

        self
    }

    /// Specifies the version number of the UUID.
    pub fn set_version(&mut self, v: crate::Version) -> &mut Self {
        self.0[6] = (self.0[6] & 0x0f) | ((v as u8) << 4);

        self
    }

    /// Hands over the internal constructed [`Uuid`].
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use uuid::Builder;
    ///
    /// let uuid = Builder::nil().build();
    ///
    /// assert_eq!(
    ///     uuid.to_hyphenated().to_string(),
    ///     "00000000-0000-0000-0000-000000000000"
    /// );
    /// ```
    ///
    /// [`Uuid`]: struct.Uuid.html
    pub fn build(&mut self) -> Uuid {
        Uuid::from_bytes(self.0)
    }
}
