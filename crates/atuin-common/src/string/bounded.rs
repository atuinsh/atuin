//! A string proven to fit a byte limit.

use super::NonNulStr;

/// A string of at most `MAX` bytes. `T` is any string type: `String`, `&str`, [`NonNulStr`], ...
#[derive(
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    derive_more::AsRef,
    derive_more::Debug,
    derive_more::Deref,
    derive_more::Display,
)]
#[as_ref(forward)]
#[debug("{_0:?}")]
#[deref(forward)]
#[display("{_0}")]
pub struct Bounded<const MAX: usize, T = String>(T);

/// The error returned when a string is over its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("string is {len} bytes, over the {max} it may hold")]
pub struct TooLong {
    pub len: usize,
    pub max: usize,
}

impl<const MAX: usize, T> Bounded<MAX, T> {
    /// The most bytes the string holds.
    pub const MAX: usize = MAX;
}

impl<const MAX: usize, T: AsRef<str>> Bounded<MAX, T> {
    /// Wrap `inner`, or fail if it is over `MAX` bytes.
    pub fn new(inner: T) -> Result<Self, TooLong> {
        match inner.as_ref().len() {
            len if len > MAX => Err(TooLong { len, max: MAX }),
            _ => Ok(Self(inner)),
        }
    }

    /// The wrapped string as a slice.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<const MAX: usize> Bounded<MAX> {
    /// `inner` cut at a char boundary to at most `MAX` bytes.
    #[must_use]
    pub fn clipping(mut inner: String) -> Self {
        inner.truncate(inner.floor_char_boundary(MAX));
        Self(inner)
    }
}

impl<const MAX: usize> Bounded<MAX, NonNulStr> {
    /// `inner` cut at a char boundary to at most `MAX` bytes.
    #[must_use]
    pub fn clipping(mut inner: NonNulStr) -> Self {
        inner.truncate(inner.floor_char_boundary(MAX));
        Self(inner)
    }
}

impl<const MAX: usize, T: Into<Self>> From<Bounded<MAX, T>> for String {
    fn from(bounded: Bounded<MAX, T>) -> Self {
        bounded.0.into()
    }
}
