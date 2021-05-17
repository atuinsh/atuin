use core::ops;
#[cfg(feature = "std")]
use std::borrow::Cow;

/// A specialized copy-on-write byte string.
///
/// The purpose of this type is to permit usage of a "borrowed or owned
/// byte string" in a way that keeps std/no-std compatibility. That is, in
/// no-std mode, this type devolves into a simple &[u8] with no owned variant
/// availble.
#[derive(Clone, Debug)]
pub struct CowBytes<'a>(Imp<'a>);

#[cfg(feature = "std")]
#[derive(Clone, Debug)]
struct Imp<'a>(Cow<'a, [u8]>);

#[cfg(not(feature = "std"))]
#[derive(Clone, Debug)]
struct Imp<'a>(&'a [u8]);

impl<'a> ops::Deref for CowBytes<'a> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<'a> CowBytes<'a> {
    /// Create a new borrowed CowBytes.
    pub fn new<B: ?Sized + AsRef<[u8]>>(bytes: &'a B) -> CowBytes<'a> {
        CowBytes(Imp::new(bytes.as_ref()))
    }

    /// Create a new owned CowBytes.
    #[cfg(feature = "std")]
    pub fn new_owned(bytes: Vec<u8>) -> CowBytes<'static> {
        CowBytes(Imp(Cow::Owned(bytes)))
    }

    /// Return a borrowed byte string, regardless of whether this is an owned
    /// or borrowed byte string internally.
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Return an owned version of this copy-on-write byte string.
    ///
    /// If this is already an owned byte string internally, then this is a
    /// no-op. Otherwise, the internal byte string is copied.
    #[cfg(feature = "std")]
    pub fn into_owned(self) -> CowBytes<'static> {
        match (self.0).0 {
            Cow::Borrowed(b) => CowBytes::new_owned(b.to_vec()),
            Cow::Owned(b) => CowBytes::new_owned(b),
        }
    }
}

impl<'a> Imp<'a> {
    #[cfg(feature = "std")]
    pub fn new(bytes: &'a [u8]) -> Imp<'a> {
        Imp(Cow::Borrowed(bytes))
    }

    #[cfg(not(feature = "std"))]
    pub fn new(bytes: &'a [u8]) -> Imp<'a> {
        Imp(bytes)
    }

    #[cfg(feature = "std")]
    pub fn as_slice(&self) -> &[u8] {
        match self.0 {
            Cow::Owned(ref x) => x,
            Cow::Borrowed(x) => x,
        }
    }

    #[cfg(not(feature = "std"))]
    pub fn as_slice(&self) -> &[u8] {
        self.0
    }
}
