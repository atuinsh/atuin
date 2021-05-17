//! Helper module to enable serializing bytes more efficiently

use core::{ops, fmt, char, iter, slice};
use core::fmt::Write;

use ser;

#[cfg(any(feature = "std", feature = "collections"))]
pub use self::bytebuf::{ByteBuf, ByteBufVisitor};

#[cfg(feature = "collections")]
use collections::Vec;

///////////////////////////////////////////////////////////////////////////////

/// `Bytes` wraps a `&[u8]` in order to serialize into a byte array.
#[derive(Clone, Copy, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Bytes<'a> {
    bytes: &'a [u8],
}

impl<'a> Bytes<'a> {
    /// Wrap an existing `&[u8]`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Bytes {
            bytes: bytes,
        }
    }
}

impl<'a> fmt::Debug for Bytes<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(f.write_str("b\""));
        for c in escape_bytestring(self.bytes) {
            try!(f.write_char(c));
        }
        f.write_char('"')
    }
}

impl<'a> From<&'a [u8]> for Bytes<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Bytes::new(bytes)
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
impl<'a> From<&'a Vec<u8>> for Bytes<'a> {
    fn from(bytes: &'a Vec<u8>) -> Self {
        Bytes::new(bytes)
    }
}

impl<'a> Into<&'a [u8]> for Bytes<'a> {
    fn into(self) -> &'a [u8] {
        self.bytes
    }
}

impl<'a> ops::Deref for Bytes<'a> {
    type Target = [u8];

    fn deref(&self) -> &[u8] { self.bytes }
}

impl<'a> ser::Serialize for Bytes<'a> {
    #[inline]
    fn serialize<S>(&self, serializer: &mut S) -> Result<(), S::Error>
        where S: ser::Serializer
    {
        serializer.serialize_bytes(self.bytes)
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(any(feature = "std", feature = "collections"))]
mod bytebuf {
    use core::ops;
    use core::fmt;
    use core::fmt::Write;

    use ser;
    use de;

    #[cfg(feature = "collections")]
    use collections::{String, Vec};

    /// `ByteBuf` wraps a `Vec<u8>` and serializes as a byte array.
    #[derive(Clone, Default, Eq, Hash, PartialEq, PartialOrd, Ord)]
    pub struct ByteBuf {
        bytes: Vec<u8>,
    }

    impl ByteBuf {
        /// Construct a new, empty `ByteBuf`.
        pub fn new() -> Self {
            ByteBuf::from(Vec::new())
        }

        /// Construct a new, empty `ByteBuf` with the specified capacity.
        pub fn with_capacity(cap: usize) -> Self {
            ByteBuf::from(Vec::with_capacity(cap))
        }

        /// Wrap existing bytes in a `ByteBuf`.
        pub fn from<T: Into<Vec<u8>>>(bytes: T) -> Self {
            ByteBuf {
                bytes: bytes.into(),
            }
        }
    }

    impl fmt::Debug for ByteBuf {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            try!(f.write_str("b\""));
            for c in super::escape_bytestring(self.bytes.as_ref()) {
                try!(f.write_char(c));
            }
            f.write_char('"')
        }
    }

    impl Into<Vec<u8>> for ByteBuf {
        fn into(self) -> Vec<u8> {
            self.bytes
        }
    }

    impl From<Vec<u8>> for ByteBuf {
        fn from(bytes: Vec<u8>) -> Self {
            ByteBuf::from(bytes)
        }
    }

    impl AsRef<Vec<u8>> for ByteBuf {
        fn as_ref(&self) -> &Vec<u8> {
            &self.bytes
        }
    }

    impl AsRef<[u8]> for ByteBuf {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl AsMut<Vec<u8>> for ByteBuf {
        fn as_mut(&mut self) -> &mut Vec<u8> {
            &mut self.bytes
        }
    }

    impl AsMut<[u8]> for ByteBuf {
        fn as_mut(&mut self) -> &mut [u8] {
            &mut self.bytes
        }
    }

    impl ops::Deref for ByteBuf {
        type Target = [u8];

        fn deref(&self) -> &[u8] { &self.bytes[..] }
    }

    impl ops::DerefMut for ByteBuf {
        fn deref_mut(&mut self) -> &mut [u8] { &mut self.bytes[..] }
    }

    impl ser::Serialize for ByteBuf {
        fn serialize<S>(&self, serializer: &mut S) -> Result<(), S::Error>
            where S: ser::Serializer
        {
            serializer.serialize_bytes(self)
        }
    }

    /// This type implements the `serde::de::Visitor` trait for a `ByteBuf`.
    pub struct ByteBufVisitor;

    impl de::Visitor for ByteBufVisitor {
        type Value = ByteBuf;

        #[inline]
        fn visit_unit<E>(&mut self) -> Result<ByteBuf, E>
            where E: de::Error,
        {
            Ok(ByteBuf::new())
        }

        #[inline]
        fn visit_seq<V>(&mut self, mut visitor: V) -> Result<ByteBuf, V::Error>
            where V: de::SeqVisitor,
        {
            let (len, _) = visitor.size_hint();
            let mut values = Vec::with_capacity(len);

            while let Some(value) = try!(visitor.visit()) {
                values.push(value);
            }

            try!(visitor.end());

            Ok(ByteBuf::from(values))
        }

        #[inline]
        fn visit_bytes<E>(&mut self, v: &[u8]) -> Result<ByteBuf, E>
            where E: de::Error,
        {
            Ok(ByteBuf::from(v))
        }

        #[inline]
        fn visit_byte_buf<E>(&mut self, v: Vec<u8>) -> Result<ByteBuf, E>
            where E: de::Error,
        {
            Ok(ByteBuf::from(v))
        }

        fn visit_str<E>(&mut self, v: &str) -> Result<ByteBuf, E>
            where E: de::Error,
        {
            Ok(ByteBuf::from(v))
        }

        fn visit_string<E>(&mut self, v: String) -> Result<ByteBuf, E>
            where E: de::Error,
        {
            Ok(ByteBuf::from(v))
        }
    }

    impl de::Deserialize for ByteBuf {
        #[inline]
        fn deserialize<D>(deserializer: &mut D) -> Result<ByteBuf, D::Error>
            where D: de::Deserializer
        {
            deserializer.deserialize_bytes(ByteBufVisitor)
        }
    }
}

///////////////////////////////////////////////////////////////////////////////

#[inline]
fn escape_bytestring<'a>(bytes: &'a [u8]) -> iter::FlatMap<slice::Iter<'a, u8>, char::EscapeDefault, fn(&u8) -> char::EscapeDefault> {
    fn f(b: &u8) -> char::EscapeDefault {
        char::from_u32(*b as u32).unwrap().escape_default()
    }
    bytes.iter().flat_map(f as fn(&u8) -> char::EscapeDefault)
}
