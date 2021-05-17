/// Character level parsers

/// Matches one of the provided characters.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
/// named!(simple<char>, one_of!(&b"abc"[..]));
/// assert_eq!(simple(b"a123"), Ok((&b"123"[..], 'a')));
///
/// named!(a_or_b<&str, char>, one_of!("ab汉"));
/// assert_eq!(a_or_b("汉jiosfe"), Ok(("jiosfe", '汉')));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! one_of (
  ($i:expr, $inp: expr) => ( $crate::character::streaming::one_of($inp)($i) );
);

/// Matches anything but the provided characters.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind};
/// # fn main() {
/// named!(no_letter_a<char>, none_of!(&b"abc"[..]));
/// assert_eq!(no_letter_a(b"123"), Ok((&b"23"[..], '1')));
///
/// named!(err_on_single_quote<char>, none_of!(&b"'"[..]));
/// assert_eq!(err_on_single_quote(b"'jiosfe"), Err(Err::Error(error_position!(&b"'jiosfe"[..], ErrorKind::NoneOf))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! none_of (
  ($i:expr, $inp: expr) => ( $crate::character::streaming::none_of($inp)($i) );
);

/// Matches one character: `char!(char) => &[u8] -> IResult<&[u8], char>`.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind};
/// # fn main() {
/// named!(match_letter_a<char>, char!('a'));
/// assert_eq!(match_letter_a(b"abc"), Ok((&b"bc"[..],'a')));
///
/// assert_eq!(match_letter_a(b"123cdef"), Err(Err::Error(error_position!(&b"123cdef"[..], ErrorKind::Char))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! char (
  ($i:expr, $c: expr) => ( $crate::character::streaming::char($c)($i) );
);

#[cfg(test)]
mod tests {
  use crate::error::ErrorKind;
  use crate::internal::Err;

  #[test]
  fn one_of() {
    named!(f<char>, one_of!("ab"));

    let a = &b"abcd"[..];
    assert_eq!(f(a), Ok((&b"bcd"[..], 'a')));

    let b = &b"cde"[..];
    assert_eq!(f(b), Err(Err::Error(error_position!(b, ErrorKind::OneOf))));

    named!(utf8(&str) -> char,
      one_of!("+\u{FF0B}"));

    assert!(utf8("+").is_ok());
    assert!(utf8("\u{FF0B}").is_ok());
  }

  #[test]
  fn none_of() {
    named!(f<char>, none_of!("ab"));

    let a = &b"abcd"[..];
    assert_eq!(f(a), Err(Err::Error(error_position!(a, ErrorKind::NoneOf))));

    let b = &b"cde"[..];
    assert_eq!(f(b), Ok((&b"de"[..], 'c')));
  }

  #[test]
  fn char() {
    named!(f<char>, char!('c'));

    let a = &b"abcd"[..];
    assert_eq!(f(a), Err(Err::Error(error_position!(a, ErrorKind::Char))));

    let b = &b"cde"[..];
    assert_eq!(f(b), Ok((&b"de"[..], 'c')));
  }

  #[test]
  fn char_str() {
    named!(f<&str, char>, char!('c'));

    let a = &"abcd"[..];
    assert_eq!(f(a), Err(Err::Error(error_position!(a, ErrorKind::Char))));

    let b = &"cde"[..];
    assert_eq!(f(b), Ok((&"de"[..], 'c')));
  }
}
