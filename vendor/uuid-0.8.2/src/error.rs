use crate::std::fmt;
use crate::{builder, parser};

/// A general error that can occur when working with UUIDs.
// TODO: improve the doc
// BODY: This detail should be fine for initial merge
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Error(Inner);

// TODO: write tests for Error
// BODY: not immediately blocking, but should be covered for 1.0
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Inner {
    /// An error occurred while handling [`Uuid`] bytes.
    ///
    /// See [`BytesError`]
    ///
    /// [`BytesError`]: struct.BytesError.html
    /// [`Uuid`]: struct.Uuid.html
    Build(builder::Error),

    /// An error occurred while parsing a [`Uuid`] string.
    ///
    /// See [`parser::ParseError`]
    ///
    /// [`parser::ParseError`]: parser/enum.ParseError.html
    /// [`Uuid`]: struct.Uuid.html
    Parser(parser::Error),
}

impl From<builder::Error> for Error {
    fn from(err: builder::Error) -> Self {
        Error(Inner::Build(err))
    }
}

impl From<parser::Error> for Error {
    fn from(err: parser::Error) -> Self {
        Error(Inner::Parser(err))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Inner::Build(ref err) => fmt::Display::fmt(&err, f),
            Inner::Parser(ref err) => fmt::Display::fmt(&err, f),
        }
    }
}

#[cfg(feature = "std")]
mod std_support {
    use super::*;
    use crate::std::error;

    impl error::Error for Error {
        fn source(&self) -> Option<&(dyn error::Error + 'static)> {
            match self.0 {
                Inner::Build(ref err) => Some(err),
                Inner::Parser(ref err) => Some(err),
            }
        }
    }
}

#[cfg(test)]
mod test_util {
    use super::*;

    impl Error {
        pub(crate) fn expect_parser(self) -> parser::Error {
            match self.0 {
                Inner::Parser(err) => err,
                _ => panic!("expected a `parser::Error` variant"),
            }
        }
    }
}
