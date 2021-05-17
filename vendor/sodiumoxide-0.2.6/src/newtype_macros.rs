macro_rules! newtype_clone (($newtype:ident) => (
        impl Clone for $newtype {
            fn clone(&self) -> $newtype {
                let &$newtype(v) = self;
                $newtype(v)
            }
        }

        ));

macro_rules! newtype_from_slice (($newtype:ident, $len:expr) => (
    /// `from_slice()` creates an object from a byte slice
    ///
    /// This function will fail and return `None` if the length of
    /// the byte-slice isn't equal to the length of the object
    pub fn from_slice(bs: &[u8]) -> Option<$newtype> {
        if bs.len() != $len {
            return None;
        }
        let mut n = $newtype([0; $len]);
        n.0.copy_from_slice(bs);
        Some(n)
    }
    ));

macro_rules! newtype_traits (($newtype:ident, $len:expr) => (
    impl ::std::cmp::PartialEq for $newtype {
        fn eq(&self, other: &$newtype) -> bool {
            use utils::memcmp;
            memcmp(&self.0, &other.0)
        }
    }
    impl ::std::cmp::Eq for $newtype {}

    #[cfg(feature = "serde")]
    impl ::serde::Serialize for $newtype {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where S: ::serde::Serializer
        {
            serializer.serialize_bytes(&self.as_ref())
        }
    }

    #[cfg(feature = "serde")]
    impl<'de> ::serde::Deserialize<'de> for $newtype {
        fn deserialize<D>(deserializer: D) -> Result<$newtype, D::Error>
            where D: ::serde::Deserializer<'de>
        {
            struct NewtypeVisitor;
            impl<'de> ::serde::de::Visitor<'de> for NewtypeVisitor {
                type Value = $newtype;
                fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                    write!(formatter, stringify!($newtype))
                }
                fn visit_seq<V>(self, mut visitor: V) -> Result<Self::Value, V::Error>
                    where V: ::serde::de::SeqAccess<'de>
                {
                    let mut res = $newtype([0; $len]);
                    for r in res.0.iter_mut() {
                        if let Some(value) = visitor.next_element()? {
                            *r = value;
                        }
                    }
                    Ok(res)
                }
                fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
                    where E: ::serde::de::Error
                {
                    $newtype::from_slice(v).ok_or(::serde::de::Error::invalid_length(v.len(), &self))
                }
            }
            deserializer.deserialize_bytes(NewtypeVisitor)
        }
    }

    impl AsRef<[u8]> for $newtype {
        #[inline]
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    /// Allows a user to access the byte contents of an object as a slice.
    ///
    /// WARNING: it might be tempting to do comparisons on objects
    /// by using `x[a..b] == y[a..b]`. This will open up for timing attacks
    /// when comparing for example authenticator tags. Because of this only
    /// use the comparison functions exposed by the sodiumoxide API.
    #[deprecated(since="0.2.2", note="Use the `AsRef` or `AsMut` implementation instead")]
    impl ::std::ops::Index<::std::ops::Range<usize>> for $newtype {
        type Output = [u8];
        fn index(&self, _index: ::std::ops::Range<usize>) -> &[u8] {
            self.0.index(_index)
        }
    }
    /// Allows a user to access the byte contents of an object as a slice.
    ///
    /// WARNING: it might be tempting to do comparisons on objects
    /// by using `x[..b] == y[..b]`. This will open up for timing attacks
    /// when comparing for example authenticator tags. Because of this only
    /// use the comparison functions exposed by the sodiumoxide API.
    #[deprecated(since="0.2.2", note="Use the `AsRef` or `AsMut` implementation instead")]
    impl ::std::ops::Index<::std::ops::RangeTo<usize>> for $newtype {
        type Output = [u8];
        fn index(&self, _index: ::std::ops::RangeTo<usize>) -> &[u8] {
            self.0.index(_index)
        }
    }
    /// Allows a user to access the byte contents of an object as a slice.
    ///
    /// WARNING: it might be tempting to do comparisons on objects
    /// by using `x[a..] == y[a..]`. This will open up for timing attacks
    /// when comparing for example authenticator tags. Because of this only
    /// use the comparison functions exposed by the sodiumoxide API.
    #[deprecated(since="0.2.2", note="Use the `AsRef` or `AsMut` implementation instead")]
    impl ::std::ops::Index<::std::ops::RangeFrom<usize>> for $newtype {
        type Output = [u8];
        fn index(&self, _index: ::std::ops::RangeFrom<usize>) -> &[u8] {
            self.0.index(_index)
        }
    }
    /// Allows a user to access the byte contents of an object as a slice.
    ///
    /// WARNING: it might be tempting to do comparisons on objects
    /// by using `x[] == y[]`. This will open up for timing attacks
    /// when comparing for example authenticator tags. Because of this only
    /// use the comparison functions exposed by the sodiumoxide API.
    #[deprecated(since="0.2.2", note="Use the `AsRef` or `AsMut` implementation instead")]
    impl ::std::ops::Index<::std::ops::RangeFull> for $newtype {
        type Output = [u8];
        fn index(&self, _index: ::std::ops::RangeFull) -> &[u8] {
            self.0.index(_index)
        }
    }
    ));

macro_rules! public_newtype_traits (($newtype:ident) => (
    impl ::std::cmp::PartialOrd for $newtype {
        #[inline]
        fn partial_cmp(&self,
                       other: &$newtype) -> Option<::std::cmp::Ordering> {
            ::std::cmp::PartialOrd::partial_cmp(self.as_ref(), other.as_ref())
        }
        #[inline]
        fn lt(&self, other: &$newtype) -> bool {
            ::std::cmp::PartialOrd::lt(self.as_ref(), other.as_ref())
        }
        #[inline]
        fn le(&self, other: &$newtype) -> bool {
            ::std::cmp::PartialOrd::le(self.as_ref(), other.as_ref())
        }
        #[inline]
        fn ge(&self, other: &$newtype) -> bool {
            ::std::cmp::PartialOrd::ge(self.as_ref(), other.as_ref())
        }
        #[inline]
        fn gt(&self, other: &$newtype) -> bool {
            ::std::cmp::PartialOrd::gt(self.as_ref(), other.as_ref())
        }
    }
    impl ::std::cmp::Ord for $newtype {
        #[inline]
        fn cmp(&self, other: &$newtype) -> ::std::cmp::Ordering {
            ::std::cmp::Ord::cmp(self.as_ref(), other.as_ref())
        }
    }
    impl ::std::hash::Hash for $newtype {
        fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
            ::std::hash::Hash::hash(self.as_ref(), state)
        }
    }
    ));

/// Macro used for generating newtypes of byte-arrays
///
/// Usage:
/// Generating secret datatypes, e.g. keys
///
/// ```
/// new_type! {
///     /// This is some documentation for our type
///     secret Key(KEYBYTES);
/// }
/// ```
///
/// Generating public datatypes, e.g. public keys
///
/// ```
/// new_type! {
///     /// This is some documentation for our type
///     public PublicKey(PUBLICKEYBYTES);
/// }
///
/// ```
///
/// Generating nonce types
///
/// ```
/// new_type! {
///     /// This is some documentation for our type
///     nonce Nonce(NONCEBYTES);
/// }
/// ```
macro_rules! new_type {
    ( $(#[$meta:meta])*
      secret $name:ident($bytes:expr);
      ) => (
        $(#[$meta])*
        #[must_use]
        pub struct $name(pub [u8; $bytes]);
        newtype_clone!($name);
        newtype_traits!($name, $bytes);
        impl $name {
            newtype_from_slice!($name, $bytes);
        }
        impl Drop for $name {
            fn drop(&mut self) {
                use utils::memzero;
                memzero(&mut self.0);
            }
        }
        impl ::std::fmt::Debug for $name {
            fn fmt(&self,
                   formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                // Hide secrets from debug output.
                write!(formatter, "{}(****)", stringify!($name))
            }
        }
        );
    ( $(#[$meta:meta])*
      public $name:ident($bytes:expr);
      ) => (
        $(#[$meta])*
        #[derive(Copy)]
        #[must_use]
        pub struct $name(pub [u8; $bytes]);
        newtype_clone!($name);
        newtype_traits!($name, $bytes);
        public_newtype_traits!($name);
        impl $name {
            newtype_from_slice!($name, $bytes);
        }
        impl ::std::fmt::Debug for $name {
            fn fmt(&self,
                   formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(formatter, "{}({:?})", stringify!($name), self.as_ref())
            }
        }
        );
    ( $(#[$meta:meta])*
      nonce $name:ident($bytes:expr);
      ) => (
        $(#[$meta])*
        #[derive(Copy)]
        #[must_use]
        pub struct $name(pub [u8; $bytes]);
        newtype_clone!($name);
        newtype_traits!($name, $bytes);
        public_newtype_traits!($name);
        impl $name {
            newtype_from_slice!($name, $bytes);

            /// `increment_le()` treats the nonce as an unsigned little-endian number and
            /// returns an incremented version of it.
            ///
            /// WARNING: this method does not check for arithmetic overflow. It is the callers
            /// responsibility to ensure that any given nonce value is only used once.
            /// If the caller does not do that the cryptographic primitives in sodiumoxide
            /// will not uphold any security guarantees (i.e. they will break)
            pub fn increment_le(&self) -> $name {
                let mut res = *self;
                res.increment_le_inplace();
                res
            }

            /// `increment_le_inplace()` treats the nonce as an unsigned little-endian number
            /// and increments it.
            ///
            /// WARNING: this method does not check for arithmetic overflow. It is the callers
            /// responsibility to ensure that any given nonce value is only used once.
            /// If the caller does not do that the cryptographic primitives in sodiumoxide
            /// will not uphold any security guarantees.
            pub fn increment_le_inplace(&mut self) {
                use utils::increment_le;
                increment_le(&mut self.0);
            }

        }
        impl crate::crypto::nonce::Nonce for $name {
            type Bytes = [u8; $bytes];
            fn from_bytes(bytes: Self::Bytes) -> Self {
                Self(bytes)
            }
        }
        impl ::std::fmt::Debug for $name {
            fn fmt(&self,
                   formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(formatter, "{}({:?})", stringify!($name), self.as_ref())
            }
        }
        );
}
