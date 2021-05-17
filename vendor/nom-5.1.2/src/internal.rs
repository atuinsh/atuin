//! Basic types to build the parsers

use self::Needed::*;
use crate::error::ErrorKind;

/// Holds the result of parsing functions
///
/// It depends on I, the input type, O, the output type, and E, the error type (by default u32)
///
/// The `Ok` side is a pair containing the remainder of the input (the part of the data that
/// was not parsed) and the produced value. The `Err` side contains an instance of `nom::Err`.
///
pub type IResult<I, O, E=(I,ErrorKind)> = Result<(I, O), Err<E>>;

/// Contains information on needed data if a parser returned `Incomplete`
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Needed {
  /// needs more data, but we do not know how much
  Unknown,
  /// contains the required data size
  Size(usize),
}

impl Needed {
  /// indicates if we know how many bytes we need
  pub fn is_known(&self) -> bool {
    *self != Unknown
  }

  /// Maps a `Needed` to `Needed` by applying a function to a contained `Size` value.
  #[inline]
  pub fn map<F: Fn(usize) -> usize>(self, f: F) -> Needed {
    match self {
      Unknown => Unknown,
      Size(n) => Size(f(n)),
    }
  }
}

/// The `Err` enum indicates the parser was not successful
///
/// It has three cases:
///
/// * `Incomplete` indicates that more data is needed to decide. The `Needed` enum
/// can contain how many additional bytes are necessary. If you are sure your parser
/// is working on full data, you can wrap your parser with the `complete` combinator
/// to transform that case in `Error`
/// * `Error` means some parser did not succeed, but another one might (as an example,
/// when testing different branches of an `alt` combinator)
/// * `Failure` indicates an unrecoverable error. As an example, if you recognize a prefix
/// to decide on the next parser to apply, and that parser fails, you know there's no need
/// to try other parsers, you were already in the right branch, so the data is invalid
///
#[derive(Debug, Clone, PartialEq)]
pub enum Err<E> {
  /// There was not enough data
  Incomplete(Needed),
  /// The parser had an error (recoverable)
  Error(E),
  /// The parser had an unrecoverable error: we got to the right
  /// branch and we know other branches won't work, so backtrack
  /// as fast as possible
  Failure(E),
}

impl<E> Err<E> {
  /// tests if the result is Incomplete
  pub fn is_incomplete(&self) -> bool {
    if let Err::Incomplete(_) = self {
      true
    } else {
      false
    }
  }

  /// Applies the given function to the inner error
  pub fn map<E2, F>(self, f: F) -> Err<E2>
    where F: FnOnce(E) -> E2
  {
    match self {
      Err::Incomplete(n) => Err::Incomplete(n),
      Err::Failure(t) => Err::Failure(f(t)),
      Err::Error(t) => Err::Error(f(t)),
    }
  }

  /// automatically converts between errors if the underlying type supports it
  pub fn convert<F>(e: Err<F>) -> Self
    where E: From<F>
  {
    e.map(Into::into)
  }
}

impl<T> Err<(T, ErrorKind)> {
  /// maps `Err<(T, ErrorKind)>` to `Err<(U, ErrorKind)>` with the given F: T -> U
  pub fn map_input<U, F>(self, f: F) -> Err<(U, ErrorKind)>
    where F: FnOnce(T) -> U {
    match self {
      Err::Incomplete(n) => Err::Incomplete(n),
      Err::Failure((input, k)) => Err::Failure((f(input), k)),
      Err::Error((input, k)) => Err::Error((f(input), k)),
    }
  }
}

#[cfg(feature = "std")]
impl Err<(&[u8], ErrorKind)> {
  /// Obtaining ownership
  pub fn to_owned(self) -> Err<(Vec<u8>, ErrorKind)> {
    self.map_input(ToOwned::to_owned)
  }
}

#[cfg(feature = "std")]
impl Err<(&str, ErrorKind)> {
  /// automatically converts between errors if the underlying type supports it
  pub fn to_owned(self) -> Err<(String, ErrorKind)> {
    self.map_input(ToOwned::to_owned)
  }
}

impl<E: Eq> Eq for Err<E> {}

#[cfg(feature = "std")]
use std::fmt;

#[cfg(feature = "std")]
impl<E> fmt::Display for Err<E>
where
  E: fmt::Debug,
{
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Err::Incomplete(Needed::Size(u)) => write!(f, "Parsing requires {} bytes/chars", u),
      Err::Incomplete(Needed::Unknown) => write!(f, "Parsing requires more data"),
      Err::Failure(c) => write!(f, "Parsing Failure: {:?}", c),
      Err::Error(c) => write!(f, "Parsing Error: {:?}", c),
    }
  }
}

#[cfg(feature = "std")]
use std::error::Error;

#[cfg(feature = "std")]
impl<E> Error for Err<E>
where
  E: fmt::Debug,
{
  fn source(&self) -> Option<&(dyn Error + 'static)> {
    None // no underlying error
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::ErrorKind;

  #[doc(hidden)]
  #[macro_export]
  macro_rules! assert_size (
    ($t:ty, $sz:expr) => (
      assert_eq!(crate::lib::std::mem::size_of::<$t>(), $sz);
    );
  );

  #[test]
  #[cfg(target_pointer_width = "64")]
  fn size_test() {
    assert_size!(IResult<&[u8], &[u8], (&[u8], u32)>, 40);
    assert_size!(IResult<&str, &str, u32>, 40);
    assert_size!(Needed, 16);
    assert_size!(Err<u32>, 24);
    assert_size!(ErrorKind, 1);
  }

  #[test]
  fn err_map_test() {
    let e = Err::Error(1);
    assert_eq!(e.map(|v| v + 1), Err::Error(2));
  }

}
