use crate::std::fmt;

/// The error that can occur when creating a [`Uuid`].
///
/// [`Uuid`]: struct.Uuid.html
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Error {
    expected: usize,
    found: usize,
}

impl Error {
    /// The expected number of bytes.
    #[inline]
    const fn expected(&self) -> usize {
        self.expected
    }

    /// The number of bytes found.
    #[inline]
    const fn found(&self) -> usize {
        self.found
    }

    /// Create a new [`UuidError`].
    ///
    /// [`UuidError`]: struct.UuidError.html
    #[inline]
    pub(crate) const fn new(expected: usize, found: usize) -> Self {
        Error { expected, found }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid bytes length: expected {}, found {}",
            self.expected(),
            self.found()
        )
    }
}

#[cfg(feature = "std")]
mod std_support {
    use super::*;

    use crate::std::error;

    impl error::Error for Error {}
}
