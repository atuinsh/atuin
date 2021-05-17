use crate::std::fmt;

/// An error that can occur while parsing a [`Uuid`] string.
///
/// [`Uuid`]: ../struct.Uuid.html
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Error {
    /// Invalid character in the [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    InvalidCharacter {
        /// The expected characters.
        expected: &'static str,
        /// The invalid character found.
        found: char,
        /// The invalid character position.
        index: usize,
        /// Indicates the [`Uuid`] starts with `urn:uuid:`.
        ///
        /// This is a special case for [`Urn`] adapter parsing.
        ///
        /// [`Uuid`]: ../Uuid.html
        urn: UrnPrefix,
    },
    /// Invalid number of segments in the [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    InvalidGroupCount {
        /// The expected number of segments.
        // TODO: explain multiple segment count.
        // BODY: Parsers can expect a range of Uuid segment count.
        //       This needs to be expanded on.
        expected: ExpectedLength,
        /// The number of segments found.
        found: usize,
    },
    /// Invalid length of a segment in a [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    InvalidGroupLength {
        /// The expected length of the segment.
        expected: ExpectedLength,
        /// The length of segment found.
        found: usize,
        /// The segment with invalid length.
        group: usize,
    },
    /// Invalid length of the [`Uuid`] string.
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    InvalidLength {
        /// The expected length(s).
        // TODO: explain multiple lengths.
        // BODY: Parsers can expect a range of Uuid lenghts.
        //       This needs to be expanded on.
        expected: ExpectedLength,
        /// The invalid length found.
        found: usize,
    },
}

/// The expected length.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum ExpectedLength {
    /// Expected any one of the given values.
    Any(&'static [usize]),
    /// Expected the given value.
    Exact(usize),
}

/// Urn prefix value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum UrnPrefix {
    /// The `urn:uuid:` prefix should optionally provided.
    Optional,
}

impl Error {
    fn _description(&self) -> &str {
        match *self {
            Error::InvalidCharacter { .. } => "invalid character",
            Error::InvalidGroupCount { .. } => "invalid number of groups",
            Error::InvalidGroupLength { .. } => "invalid group length",
            Error::InvalidLength { .. } => "invalid length",
        }
    }
}

impl fmt::Display for ExpectedLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ExpectedLength::Any(crits) => write!(f, "one of {:?}", crits),
            ExpectedLength::Exact(crit) => write!(f, "{}", crit),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self._description())?;

        match *self {
            Error::InvalidCharacter {
                expected,
                found,
                index,
                urn,
            } => {
                let urn_str = match urn {
                    UrnPrefix::Optional => {
                        " an optional prefix of `urn:uuid:` followed by"
                    }
                };

                write!(
                    f,
                    "expected{} {}, found {} at {}",
                    urn_str, expected, found, index
                )
            }
            Error::InvalidGroupCount {
                ref expected,
                found,
            } => write!(f, "expected {}, found {}", expected, found),
            Error::InvalidGroupLength {
                ref expected,
                found,
                group,
            } => write!(
                f,
                "expected {}, found {} in group {}",
                expected, found, group,
            ),
            Error::InvalidLength {
                ref expected,
                found,
            } => write!(f, "expected {}, found {}", expected, found),
        }
    }
}

#[cfg(feature = "std")]
mod std_support {
    use super::*;
    use crate::std::error;

    impl error::Error for Error {}
}
