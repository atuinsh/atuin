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

//! Adapters for various formats for UUIDs

use crate::prelude::*;
use crate::std::{fmt, str};

#[cfg(feature = "serde")]
pub mod compact;

/// An adaptor for formatting an [`Uuid`] as a hyphenated string.
///
/// Takes an owned instance of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Hyphenated(Uuid);

/// An adaptor for formatting an [`Uuid`] as a hyphenated string.
///
/// Takes a reference of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HyphenatedRef<'a>(&'a Uuid);

/// An adaptor for formatting an [`Uuid`] as a simple string.
///
/// Takes an owned instance of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Simple(Uuid);

/// An adaptor for formatting an [`Uuid`] as a simple string.
///
/// Takes a reference of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SimpleRef<'a>(&'a Uuid);

/// An adaptor for formatting an [`Uuid`] as a URN string.
///
/// Takes an owned instance of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Urn(Uuid);

/// An adaptor for formatting an [`Uuid`] as a URN string.
///
/// Takes a reference of the [`Uuid`].
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UrnRef<'a>(&'a Uuid);

impl Uuid {
    /// Get a [`Hyphenated`] formatter.
    ///
    /// [`Hyphenated`]: adapter/struct.Hyphenated.html
    #[inline]
    pub const fn to_hyphenated(self) -> Hyphenated {
        Hyphenated::from_uuid(self)
    }

    /// Get a borrowed [`HyphenatedRef`] formatter.
    ///
    /// [`HyphenatedRef`]: adapter/struct.HyphenatedRef.html
    #[inline]
    pub const fn to_hyphenated_ref(&self) -> HyphenatedRef<'_> {
        HyphenatedRef::from_uuid_ref(self)
    }

    /// Get a [`Simple`] formatter.
    ///
    /// [`Simple`]: adapter/struct.Simple.html
    #[inline]
    pub const fn to_simple(self) -> Simple {
        Simple::from_uuid(self)
    }

    /// Get a borrowed [`SimpleRef`] formatter.
    ///
    /// [`SimpleRef`]: adapter/struct.SimpleRef.html
    #[inline]
    pub const fn to_simple_ref(&self) -> SimpleRef<'_> {
        SimpleRef::from_uuid_ref(self)
    }

    /// Get a [`Urn`] formatter.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`Urn`]: adapter/struct.Urn.html
    #[inline]
    pub const fn to_urn(self) -> Urn {
        Urn::from_uuid(self)
    }

    /// Get a borrowed [`UrnRef`] formatter.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`UrnRef`]: adapter/struct.UrnRef.html
    #[inline]
    pub const fn to_urn_ref(&self) -> UrnRef<'_> {
        UrnRef::from_uuid_ref(self)
    }
}

const UPPER: [u8; 16] = [
    b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'A', b'B',
    b'C', b'D', b'E', b'F',
];
const LOWER: [u8; 16] = [
    b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'a', b'b',
    b'c', b'd', b'e', b'f',
];
/// The segments of a UUID's [u8; 16] corresponding to each group.
const BYTE_POSITIONS: [usize; 6] = [0, 4, 6, 8, 10, 16];
/// The locations that hyphens are written into the buffer, after each
/// group.
const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];

/// Encodes the `uuid` possibly with hyphens, and possibly in upper
/// case, to full_buffer[start..] and returns the str sliced from
/// full_buffer[..start + encoded_length].
///
/// The `start` parameter allows writing a prefix (such as
/// "urn:uuid:") to the buffer that's included in the final encoded
/// UUID.
#[allow(clippy::needless_range_loop)]
fn encode<'a>(
    full_buffer: &'a mut [u8],
    start: usize,
    uuid: &Uuid,
    hyphens: bool,
    upper: bool,
) -> &'a mut str {
    let len = if hyphens { 36 } else { 32 };

    {
        let buffer = &mut full_buffer[start..start + len];
        let bytes = uuid.as_bytes();

        let hex = if upper { &UPPER } else { &LOWER };

        for group in 0..5 {
            // If we're writing hyphens, we need to shift the output
            // location along by how many of them have been written
            // before this point. That's exactly the (0-indexed) group
            // number.
            let hyphens_before = if hyphens { group } else { 0 };
            for idx in BYTE_POSITIONS[group]..BYTE_POSITIONS[group + 1] {
                let b = bytes[idx];
                let out_idx = hyphens_before + 2 * idx;

                buffer[out_idx] = hex[(b >> 4) as usize];
                buffer[out_idx + 1] = hex[(b & 0b1111) as usize];
            }

            if group != 4 && hyphens {
                buffer[HYPHEN_POSITIONS[group]] = b'-';
            }
        }
    }

    str::from_utf8_mut(&mut full_buffer[..start + len])
        .expect("found non-ASCII output characters while encoding a UUID")
}

impl Hyphenated {
    /// The length of a hyphenated [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 36;

    /// Creates a [`Hyphenated`] from a [`Uuid`].
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`Hyphenated`]: struct.Hyphenated.html
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Hyphenated(uuid)
    }

    /// Writes the [`Uuid`] as a lower-case hyphenated string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_hyphenated()
    ///             .encode_lower(&mut Uuid::encode_buffer()),
    ///         "936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 40];
    ///     uuid.to_hyphenated().encode_lower(&mut buf);
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936da01f-9abd-4d9d-80c7-02af85c822a8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, &self.0, true, false)
    }

    /// Writes the [`Uuid`] as an upper-case hyphenated string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_hyphenated()
    ///             .encode_upper(&mut Uuid::encode_buffer()),
    ///         "936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 40];
    ///     uuid.to_hyphenated().encode_upper(&mut buf);
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936DA01F-9ABD-4D9D-80C7-02AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, &self.0, true, true)
    }
}

impl<'a> HyphenatedRef<'a> {
    /// The length of a hyphenated [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 36;

    /// Creates a [`HyphenatedRef`] from a [`Uuid`] reference.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`HyphenatedRef`]: struct.HyphenatedRef.html
    pub const fn from_uuid_ref(uuid: &'a Uuid) -> Self {
        HyphenatedRef(uuid)
    }

    /// Writes the [`Uuid`] as a lower-case hyphenated string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_hyphenated()
    ///             .encode_lower(&mut Uuid::encode_buffer()),
    ///         "936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///     
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 40];
    ///     uuid.to_hyphenated().encode_lower(&mut buf);
    ///     assert_eq!(
    ///         uuid.to_hyphenated().encode_lower(&mut buf),
    ///         "936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936da01f-9abd-4d9d-80c7-02af85c822a8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, self.0, true, false)
    }

    /// Writes the [`Uuid`] as an upper-case hyphenated string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_hyphenated()
    ///             .encode_upper(&mut Uuid::encode_buffer()),
    ///         "936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 40];
    ///     assert_eq!(
    ///         uuid.to_hyphenated().encode_upper(&mut buf),
    ///         "936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936DA01F-9ABD-4D9D-80C7-02AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, self.0, true, true)
    }
}

impl Simple {
    /// The length of a simple [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 32;

    /// Creates a [`Simple`] from a [`Uuid`].
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`Simple`]: struct.Simple.html
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Simple(uuid)
    }

    /// Writes the [`Uuid`] as a lower-case simple string to `buffer`,
    /// and returns the subslice of the buffer that contains the encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_simple().encode_lower(&mut Uuid::encode_buffer()),
    ///         "936da01f9abd4d9d80c702af85c822a8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 36];
    ///     assert_eq!(
    ///         uuid.to_simple().encode_lower(&mut buf),
    ///         "936da01f9abd4d9d80c702af85c822a8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936da01f9abd4d9d80c702af85c822a8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, &self.0, false, false)
    }

    /// Writes the [`Uuid`] as an upper-case simple string to `buffer`,
    /// and returns the subslice of the buffer that contains the encoded UUID.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_simple().encode_upper(&mut Uuid::encode_buffer()),
    ///         "936DA01F9ABD4D9D80C702AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 36];
    ///     assert_eq!(
    ///         uuid.to_simple().encode_upper(&mut buf),
    ///         "936DA01F9ABD4D9D80C702AF85C822A8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936DA01F9ABD4D9D80C702AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, &self.0, false, true)
    }
}

impl<'a> SimpleRef<'a> {
    /// The length of a simple [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 32;

    /// Creates a [`SimpleRef`] from a [`Uuid`] reference.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`SimpleRef`]: struct.SimpleRef.html
    pub const fn from_uuid_ref(uuid: &'a Uuid) -> Self {
        SimpleRef(uuid)
    }

    /// Writes the [`Uuid`] as a lower-case simple string to `buffer`,
    /// and returns the subslice of the buffer that contains the encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_simple().encode_lower(&mut Uuid::encode_buffer()),
    ///         "936da01f9abd4d9d80c702af85c822a8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 36];
    ///     assert_eq!(
    ///         uuid.to_simple().encode_lower(&mut buf),
    ///         "936da01f9abd4d9d80c702af85c822a8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936da01f9abd4d9d80c702af85c822a8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, self.0, false, false)
    }

    /// Writes the [`Uuid`] as an upper-case simple string to `buffer`,
    /// and returns the subslice of the buffer that contains the encoded UUID.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_simple().encode_upper(&mut Uuid::encode_buffer()),
    ///         "936DA01F9ABD4D9D80C702AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 36];
    ///     assert_eq!(
    ///         uuid.to_simple().encode_upper(&mut buf),
    ///         "936DA01F9ABD4D9D80C702AF85C822A8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"936DA01F9ABD4D9D80C702AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        encode(buffer, 0, self.0, false, true)
    }
}

impl Urn {
    /// The length of a URN [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 45;

    /// Creates a [`Urn`] from a [`Uuid`].
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`Urn`]: struct.Urn.html
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Urn(uuid)
    }

    /// Writes the [`Uuid`] as a lower-case URN string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_urn().encode_lower(&mut Uuid::encode_buffer()),
    ///         "urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 49];
    ///     uuid.to_urn().encode_lower(&mut buf);
    ///     assert_eq!(
    ///         uuid.to_urn().encode_lower(&mut buf),
    ///         "urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8!!!!" as &[_]
    ///     );
    ///     
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        buffer[..9].copy_from_slice(b"urn:uuid:");
        encode(buffer, 9, &self.0, true, false)
    }

    /// Writes the [`Uuid`] as an upper-case URN string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_urn().encode_upper(&mut Uuid::encode_buffer()),
    ///         "urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 49];
    ///     assert_eq!(
    ///         uuid.to_urn().encode_upper(&mut buf),
    ///         "urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        buffer[..9].copy_from_slice(b"urn:uuid:");
        encode(buffer, 9, &self.0, true, true)
    }
}

impl<'a> UrnRef<'a> {
    /// The length of a URN [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub const LENGTH: usize = 45;

    /// Creates a [`UrnRef`] from a [`Uuid`] reference.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    /// [`UrnRef`]: struct.UrnRef.html
    pub const fn from_uuid_ref(uuid: &'a Uuid) -> Self {
        UrnRef(&uuid)
    }

    /// Writes the [`Uuid`] as a lower-case URN string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936DA01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_urn().encode_lower(&mut Uuid::encode_buffer()),
    ///         "urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 49];
    ///     uuid.to_urn().encode_lower(&mut buf);
    ///     assert_eq!(
    ///         uuid.to_urn().encode_lower(&mut buf),
    ///         "urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"urn:uuid:936da01f-9abd-4d9d-80c7-02af85c822a8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_lower<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        buffer[..9].copy_from_slice(b"urn:uuid:");
        encode(buffer, 9, self.0, true, false)
    }

    /// Writes the [`Uuid`] as an upper-case URN string to
    /// `buffer`, and returns the subslice of the buffer that contains the
    /// encoded UUID.
    ///
    /// This is slightly more efficient than using the formatting
    /// infrastructure as it avoids virtual calls, and may avoid
    /// double buffering.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    ///
    /// # Panics
    ///
    /// Panics if the buffer is not large enough: it must have length at least
    /// [`LENGTH`]. [`Uuid::encode_buffer`] can be used to get a
    /// sufficiently-large temporary buffer.
    ///
    /// [`LENGTH`]: #associatedconstant.LENGTH
    /// [`Uuid::encode_buffer`]: ../struct.Uuid.html#method.encode_buffer
    ///
    /// # Examples
    ///
    /// ```rust
    /// use uuid::Uuid;
    ///
    /// fn main() -> Result<(), uuid::Error> {
    ///     let uuid = Uuid::parse_str("936da01f9abd4d9d80c702af85c822a8")?;
    ///
    ///     // the encoded portion is returned
    ///     assert_eq!(
    ///         uuid.to_urn().encode_upper(&mut Uuid::encode_buffer()),
    ///         "urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///
    ///     // the buffer is mutated directly, and trailing contents remains
    ///     let mut buf = [b'!'; 49];
    ///     assert_eq!(
    ///         uuid.to_urn().encode_upper(&mut buf),
    ///         "urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8"
    ///     );
    ///     assert_eq!(
    ///         &buf as &[_],
    ///         b"urn:uuid:936DA01F-9ABD-4D9D-80C7-02AF85C822A8!!!!" as &[_]
    ///     );
    ///
    ///     Ok(())
    /// }
    /// ```
    /// */
    pub fn encode_upper<'buf>(&self, buffer: &'buf mut [u8]) -> &'buf mut str {
        buffer[..9].copy_from_slice(b"urn:uuid:");
        encode(buffer, 9, self.0, true, true)
    }
}

macro_rules! impl_adapter_traits {
    ($($T:ident<$($a:lifetime),*>),+) => {$(
        impl<$($a),*> fmt::Display for $T<$($a),*> {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::LowerHex::fmt(self, f)
            }
        }

        impl<$($a),*> fmt::LowerHex for $T<$($a),*> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // TODO: Self doesn't work https://github.com/rust-lang/rust/issues/52808
                f.write_str(self.encode_lower(&mut [0; $T::LENGTH]))
            }
        }

        impl<$($a),*> fmt::UpperHex for $T<$($a),*> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // TODO: Self doesn't work https://github.com/rust-lang/rust/issues/52808
                f.write_str(self.encode_upper(&mut [0; $T::LENGTH]))
            }
        }

        impl_adapter_from!($T<$($a),*>);
    )+}
}

macro_rules! impl_adapter_from {
    ($T:ident<>) => {
        impl From<Uuid> for $T {
            #[inline]
            fn from(f: Uuid) -> Self {
                $T::from_uuid(f)
            }
        }
    };
    ($T:ident<$a:lifetime>) => {
        impl<$a> From<&$a Uuid> for $T<$a> {
            #[inline]
            fn from(f: &$a Uuid) -> Self {
                $T::from_uuid_ref(f)
            }
        }
    };
}

impl_adapter_traits! {
    Hyphenated<>,
    HyphenatedRef<'a>,
    Simple<>,
    SimpleRef<'a>,
    Urn<>,
    UrnRef<'a>
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    #[test]
    fn hyphenated_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_hyphenated().encode_lower(&mut buf).len();
        assert_eq!(len, super::Hyphenated::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    fn hyphenated_ref_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_hyphenated().encode_lower(&mut buf).len();
        assert_eq!(len, super::HyphenatedRef::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    fn simple_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_simple().encode_lower(&mut buf).len();
        assert_eq!(len, super::Simple::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    fn simple_ref_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_simple().encode_lower(&mut buf).len();
        assert_eq!(len, super::SimpleRef::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    fn urn_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_urn().encode_lower(&mut buf).len();
        assert_eq!(len, super::Urn::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    fn urn_ref_trailing() {
        let mut buf = [b'x'; 100];
        let len = Uuid::nil().to_urn().encode_lower(&mut buf).len();
        assert_eq!(len, super::UrnRef::LENGTH);
        assert!(buf[len..].iter().all(|x| *x == b'x'));
    }

    #[test]
    #[should_panic]
    fn hyphenated_too_small() {
        Uuid::nil().to_hyphenated().encode_lower(&mut [0; 35]);
    }

    #[test]
    #[should_panic]
    fn hyphenated_ref_too_small() {
        Uuid::nil().to_hyphenated_ref().encode_lower(&mut [0; 35]);
    }

    #[test]
    #[should_panic]
    fn simple_too_small() {
        Uuid::nil().to_simple().encode_lower(&mut [0; 31]);
    }
    #[test]
    #[should_panic]
    fn simple_ref_too_small() {
        Uuid::nil().to_simple_ref().encode_lower(&mut [0; 31]);
    }
    #[test]
    #[should_panic]
    fn urn_too_small() {
        Uuid::nil().to_urn().encode_lower(&mut [0; 44]);
    }
    #[test]
    #[should_panic]
    fn urn_ref_too_small() {
        Uuid::nil().to_urn_ref().encode_lower(&mut [0; 44]);
    }
}
