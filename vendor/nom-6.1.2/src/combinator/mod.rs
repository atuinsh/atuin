//! General purpose combinators

#![allow(unused_imports)]

#[cfg(feature = "alloc")]
use crate::lib::std::boxed::Box;

use crate::error::{ErrorKind, FromExternalError, ParseError};
use crate::internal::*;
use crate::lib::std::borrow::Borrow;
use crate::lib::std::convert::Into;
#[cfg(feature = "std")]
use crate::lib::std::fmt::Debug;
use crate::lib::std::mem::transmute;
use crate::lib::std::ops::{Range, RangeFrom, RangeTo};
use crate::traits::{AsChar, InputIter, InputLength, InputTakeAtPosition, ParseTo};
use crate::traits::{Compare, CompareResult, Offset, Slice};

#[macro_use]
mod macros;

/// Return the remaining input.
///
/// ```rust
/// # use nom::error::ErrorKind;
/// use nom::combinator::rest;
/// assert_eq!(rest::<_,(_, ErrorKind)>("abc"), Ok(("", "abc")));
/// assert_eq!(rest::<_,(_, ErrorKind)>(""), Ok(("", "")));
/// ```
#[inline]
pub fn rest<T, E: ParseError<T>>(input: T) -> IResult<T, T, E>
where
  T: Slice<RangeFrom<usize>>,
  T: InputLength,
{
  Ok((input.slice(input.input_len()..), input))
}

/// Return the length of the remaining input.
///
/// ```rust
/// # use nom::error::ErrorKind;
/// use nom::combinator::rest_len;
/// assert_eq!(rest_len::<_,(_, ErrorKind)>("abc"), Ok(("abc", 3)));
/// assert_eq!(rest_len::<_,(_, ErrorKind)>(""), Ok(("", 0)));
/// ```
#[inline]
pub fn rest_len<T, E: ParseError<T>>(input: T) -> IResult<T, usize, E>
where
  T: InputLength,
{
  let len = input.input_len();
  Ok((input, len))
}

/// Maps a function on the result of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult,Parser};
/// use nom::character::complete::digit1;
/// use nom::combinator::map;
/// # fn main() {
///
/// let mut parser = map(digit1, |s: &str| s.len());
///
/// // the parser will count how many characters were returned by digit1
/// assert_eq!(parser.parse("123456"), Ok(("", 6)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parser.parse("abc"), Err(Err::Error(("abc", ErrorKind::Digit))));
/// # }
/// ```
pub fn map<I, O1, O2, E, F, G>(mut first: F, mut second: G) -> impl FnMut(I) -> IResult<I, O2, E>
where
  F: Parser<I, O1, E>,
  G: FnMut(O1) -> O2,
{
  move |input: I| {
    let (input, o1) = first.parse(input)?;
    Ok((input, second(o1)))
  }
}

#[doc(hidden)]
pub fn mapc<I, O1, O2, E, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> O2,
{
  map(first, second).parse(input)
}

/// Applies a function returning a `Result` over the result of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::combinator::map_res;
/// # fn main() {
///
/// let mut parse = map_res(digit1, |s: &str| s.parse::<u8>());
///
/// // the parser will convert the result of digit1 to a number
/// assert_eq!(parse("123"), Ok(("", 123)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parse("abc"), Err(Err::Error(("abc", ErrorKind::Digit))));
///
/// // this will fail if the mapped function fails (a `u8` is too small to hold `123456`)
/// assert_eq!(parse("123456"), Err(Err::Error(("123456", ErrorKind::MapRes))));
/// # }
/// ```
pub fn map_res<I: Clone, O1, O2, E: FromExternalError<I, E2>, E2, F, G>(
  mut first: F,
  mut second: G,
) -> impl FnMut(I) -> IResult<I, O2, E>
where
  F: Parser<I, O1, E>,
  G: FnMut(O1) -> Result<O2, E2>,
{
  move |input: I| {
    let i = input.clone();
    let (input, o1) = first.parse(input)?;
    match second(o1) {
      Ok(o2) => Ok((input, o2)),
      Err(e) => Err(Err::Error(E::from_external_error(i, ErrorKind::MapRes, e))),
    }
  }
}

#[doc(hidden)]
pub fn map_resc<I: Clone, O1, O2, E: FromExternalError<I, E2>, E2, F, G>(
  input: I,
  first: F,
  second: G,
) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Result<O2, E2>,
{
  map_res(first, second)(input)
}

/// Applies a function returning an `Option` over the result of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::combinator::map_opt;
/// # fn main() {
///
/// let mut parse = map_opt(digit1, |s: &str| s.parse::<u8>().ok());
///
/// // the parser will convert the result of digit1 to a number
/// assert_eq!(parse("123"), Ok(("", 123)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parse("abc"), Err(Err::Error(("abc", ErrorKind::Digit))));
///
/// // this will fail if the mapped function fails (a `u8` is too small to hold `123456`)
/// assert_eq!(parse("123456"), Err(Err::Error(("123456", ErrorKind::MapOpt))));
/// # }
/// ```
pub fn map_opt<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  mut first: F,
  mut second: G,
) -> impl FnMut(I) -> IResult<I, O2, E>
where
  F: Parser<I, O1, E>,
  G: FnMut(O1) -> Option<O2>,
{
  move |input: I| {
    let i = input.clone();
    let (input, o1) = first.parse(input)?;
    match second(o1) {
      Some(o2) => Ok((input, o2)),
      None => Err(Err::Error(E::from_error_kind(i, ErrorKind::MapOpt))),
    }
  }
}

#[doc(hidden)]
pub fn map_optc<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  input: I,
  first: F,
  second: G,
) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Option<O2>,
{
  map_opt(first, second)(input)
}

/// Applies a parser over the result of another one.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::bytes::complete::take;
/// use nom::combinator::map_parser;
/// # fn main() {
///
/// let mut parse = map_parser(take(5u8), digit1);
///
/// assert_eq!(parse("12345"), Ok(("", "12345")));
/// assert_eq!(parse("123ab"), Ok(("", "123")));
/// assert_eq!(parse("123"), Err(Err::Error(("123", ErrorKind::Eof))));
/// # }
/// ```
pub fn map_parser<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  mut first: F,
  mut second: G,
) -> impl FnMut(I) -> IResult<I, O2, E>
where
  F: Parser<I, O1, E>,
  G: Parser<O1, O2, E>,
  O1: InputLength,
{
  move |input: I| {
    let (input, o1) = first.parse(input)?;
    let (_, o2) = second.parse(o1)?;
    Ok((input, o2))
  }
}

#[doc(hidden)]
pub fn map_parserc<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  input: I,
  first: F,
  second: G,
) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> IResult<O1, O2, E>,
  O1: InputLength,
{
  map_parser(first, second)(input)
}

/// Creates a new parser from the output of the first parser, then apply that parser over the rest of the input.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::bytes::complete::take;
/// use nom::number::complete::u8;
/// use nom::combinator::flat_map;
/// # fn main() {
///
/// let mut parse = flat_map(u8, take);
///
/// assert_eq!(parse(&[2, 0, 1, 2][..]), Ok((&[2][..], &[0, 1][..])));
/// assert_eq!(parse(&[4, 0, 1, 2][..]), Err(Err::Error((&[0, 1, 2][..], ErrorKind::Eof))));
/// # }
/// ```
pub fn flat_map<I, O1, O2, E: ParseError<I>, F, G, H>(
  mut first: F,
  second: G,
) -> impl FnMut(I) -> IResult<I, O2, E>
where
  F: Parser<I, O1, E>,
  G: Fn(O1) -> H,
  H: Parser<I, O2, E>,
{
  move |input: I| {
    let (input, o1) = first.parse(input)?;
    second(o1).parse(input)
  }
}

/// Optional parser: Will return `None` if not successful.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::opt;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// fn parser(i: &str) -> IResult<&str, Option<&str>> {
///   opt(alpha1)(i)
/// }
///
/// assert_eq!(parser("abcd;"), Ok((";", Some("abcd"))));
/// assert_eq!(parser("123;"), Ok(("123;", None)));
/// # }
/// ```
pub fn opt<I: Clone, O, E: ParseError<I>, F>(mut f: F) -> impl FnMut(I) -> IResult<I, Option<O>, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f.parse(input) {
      Ok((i, o)) => Ok((i, Some(o))),
      Err(Err::Error(_)) => Ok((i, None)),
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn optc<I: Clone, O, E: ParseError<I>, F>(input: I, f: F) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  opt(f)(input)
}

/// Calls the parser if the condition is met.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::{Error, ErrorKind}, IResult};
/// use nom::combinator::cond;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// fn parser(b: bool, i: &str) -> IResult<&str, Option<&str>> {
///   cond(b, alpha1)(i)
/// }
///
/// assert_eq!(parser(true, "abcd;"), Ok((";", Some("abcd"))));
/// assert_eq!(parser(false, "abcd;"), Ok(("abcd;", None)));
/// assert_eq!(parser(true, "123;"), Err(Err::Error(Error::new("123;", ErrorKind::Alpha))));
/// assert_eq!(parser(false, "123;"), Ok(("123;", None)));
/// # }
/// ```
pub fn cond<I, O, E: ParseError<I>, F>(
  b: bool,
  mut f: F,
) -> impl FnMut(I) -> IResult<I, Option<O>, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    if b {
      match f.parse(input) {
        Ok((i, o)) => Ok((i, Some(o))),
        Err(e) => Err(e),
      }
    } else {
      Ok((input, None))
    }
  }
}

#[doc(hidden)]
pub fn condc<I, O, E: ParseError<I>, F>(input: I, b: bool, f: F) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  cond(b, f)(input)
}

/// Tries to apply its parser without consuming the input.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::peek;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = peek(alpha1);
///
/// assert_eq!(parser("abcd;"), Ok(("abcd;", "abcd")));
/// assert_eq!(parser("123;"), Err(Err::Error(("123;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn peek<I: Clone, O, E: ParseError<I>, F>(mut f: F) -> impl FnMut(I) -> IResult<I, O, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f.parse(input) {
      Ok((_, o)) => Ok((i, o)),
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn peekc<I: Clone, O, E: ParseError<I>, F>(input: I, f: F) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  peek(f)(input)
}

/// returns its input if it is at the end of input data
///
/// When we're at the end of the data, this combinator
/// will succeed
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use std::str;
/// # use nom::{Err, error::ErrorKind, IResult};
/// # use nom::combinator::eof;
///
/// # fn main() {
/// let parser = eof;
/// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::Eof))));
/// assert_eq!(parser(""), Ok(("", "")));
/// # }
/// ```
pub fn eof<I: InputLength + Clone, E: ParseError<I>>(input: I) -> IResult<I, I, E> {
  if input.input_len() == 0 {
    let clone = input.clone();
    Ok((input, clone))
  } else {
    Err(Err::Error(E::from_error_kind(input, ErrorKind::Eof)))
  }
}

/// Transforms Incomplete into `Error`.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::bytes::streaming::take;
/// use nom::combinator::complete;
/// # fn main() {
///
/// let mut parser = complete(take(5u8));
///
/// assert_eq!(parser("abcdefg"), Ok(("fg", "abcde")));
/// assert_eq!(parser("abcd"), Err(Err::Error(("abcd", ErrorKind::Complete))));
/// # }
/// ```
pub fn complete<I: Clone, O, E: ParseError<I>, F>(mut f: F) -> impl FnMut(I) -> IResult<I, O, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f.parse(input) {
      Err(Err::Incomplete(_)) => Err(Err::Error(E::from_error_kind(i, ErrorKind::Complete))),
      rest => rest,
    }
  }
}

#[doc(hidden)]
pub fn completec<I: Clone, O, E: ParseError<I>, F>(input: I, f: F) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  complete(f)(input)
}

/// Succeeds if all the input has been consumed by its child parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::all_consuming;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = all_consuming(alpha1);
///
/// assert_eq!(parser("abcd"), Ok(("", "abcd")));
/// assert_eq!(parser("abcd;"),Err(Err::Error((";", ErrorKind::Eof))));
/// assert_eq!(parser("123abcd;"),Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn all_consuming<I, O, E: ParseError<I>, F>(mut f: F) -> impl FnMut(I) -> IResult<I, O, E>
where
  I: InputLength,
  F: Parser<I, O, E>,
{
  move |input: I| {
    let (input, res) = f.parse(input)?;
    if input.input_len() == 0 {
      Ok((input, res))
    } else {
      Err(Err::Error(E::from_error_kind(input, ErrorKind::Eof)))
    }
  }
}

/// Returns the result of the child parser if it satisfies a verification function.
///
/// The verification function takes as argument a reference to the output of the
/// parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::verify;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = verify(alpha1, |s: &str| s.len() == 4);
///
/// assert_eq!(parser("abcd"), Ok(("", "abcd")));
/// assert_eq!(parser("abcde"), Err(Err::Error(("abcde", ErrorKind::Verify))));
/// assert_eq!(parser("123abcd;"),Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn verify<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  mut first: F,
  second: G,
) -> impl FnMut(I) -> IResult<I, O1, E>
where
  F: Parser<I, O1, E>,
  G: Fn(&O2) -> bool,
  O1: Borrow<O2>,
  O2: ?Sized,
{
  move |input: I| {
    let i = input.clone();
    let (input, o) = first.parse(input)?;

    if second(o.borrow()) {
      Ok((input, o))
    } else {
      Err(Err::Error(E::from_error_kind(i, ErrorKind::Verify)))
    }
  }
}

#[doc(hidden)]
pub fn verifyc<I: Clone, O1, O2, E: ParseError<I>, F, G>(
  input: I,
  first: F,
  second: G,
) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(&O2) -> bool,
  O1: Borrow<O2>,
  O2: ?Sized,
{
  verify(first, second)(input)
}

/// Returns the provided value if the child parser succeeds.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::value;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = value(1234, alpha1);
///
/// assert_eq!(parser("abcd"), Ok(("", 1234)));
/// assert_eq!(parser("123abcd;"), Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn value<I, O1: Clone, O2, E: ParseError<I>, F>(
  val: O1,
  mut parser: F,
) -> impl FnMut(I) -> IResult<I, O1, E>
where
  F: Parser<I, O2, E>,
{
  move |input: I| parser.parse(input).map(|(i, _)| (i, val.clone()))
}

#[doc(hidden)]
pub fn valuec<I, O1: Clone, O2, E: ParseError<I>, F>(
  input: I,
  val: O1,
  parser: F,
) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O2, E>,
{
  value(val, parser)(input)
}

/// Succeeds if the child parser returns an error.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::not;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = not(alpha1);
///
/// assert_eq!(parser("123"), Ok(("123", ())));
/// assert_eq!(parser("abcd"), Err(Err::Error(("abcd", ErrorKind::Not))));
/// # }
/// ```
pub fn not<I: Clone, O, E: ParseError<I>, F>(mut parser: F) -> impl FnMut(I) -> IResult<I, (), E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser.parse(input) {
      Ok(_) => Err(Err::Error(E::from_error_kind(i, ErrorKind::Not))),
      Err(Err::Error(_)) => Ok((i, ())),
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn notc<I: Clone, O, E: ParseError<I>, F>(input: I, parser: F) -> IResult<I, (), E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  not(parser)(input)
}

/// If the child parser was successful, return the consumed input as produced value.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::recognize;
/// use nom::character::complete::{char, alpha1};
/// use nom::sequence::separated_pair;
/// # fn main() {
///
/// let mut parser = recognize(separated_pair(alpha1, char(','), alpha1));
///
/// assert_eq!(parser("abcd,efgh"), Ok(("", "abcd,efgh")));
/// assert_eq!(parser("abcd;"),Err(Err::Error((";", ErrorKind::Char))));
/// # }
/// ```
pub fn recognize<I: Clone + Offset + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(
  mut parser: F,
) -> impl FnMut(I) -> IResult<I, I, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser.parse(i) {
      Ok((i, _)) => {
        let index = input.offset(&i);
        Ok((i, input.slice(..index)))
      }
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn recognizec<I: Clone + Offset + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(
  input: I,
  parser: F,
) -> IResult<I, I, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  recognize(parser)(input)
}

/// if the child parser was successful, return the consumed input with the output
/// as a tuple. Functions similarly to [recognize](fn.recognize.html) except it
/// returns the parser output as well.
///
/// This can be useful especially in cases where the output is not the same type
/// as the input, or the input is a user defined type.
///
/// Returned tuple is of the format `(consumed input, produced output)`.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::{consumed, value, recognize, map};
/// use nom::character::complete::{char, alpha1};
/// use nom::bytes::complete::tag;
/// use nom::sequence::separated_pair;
///
/// fn inner_parser(input: &str) -> IResult<&str, bool> {
///     value(true, tag("1234"))(input)
/// }
///
/// # fn main() {
///
/// let mut consumed_parser = consumed(value(true, separated_pair(alpha1, char(','), alpha1)));
///
/// assert_eq!(consumed_parser("abcd,efgh1"), Ok(("1", ("abcd,efgh", true))));
/// assert_eq!(consumed_parser("abcd;"),Err(Err::Error((";", ErrorKind::Char))));
///
///
/// // the first output (representing the consumed input)
/// // should be the same as that of the `recognize` parser.
/// let mut recognize_parser = recognize(inner_parser);
/// let mut consumed_parser = map(consumed(inner_parser), |(consumed, output)| consumed);
///
/// assert_eq!(recognize_parser("1234"), consumed_parser("1234"));
/// assert_eq!(recognize_parser("abcd"), consumed_parser("abcd"));
/// # }
/// ```
pub fn consumed<I, O, F, E>(mut parser: F) -> impl FnMut(I) -> IResult<I, (I, O), E>
where
  I: Clone + Offset + Slice<RangeTo<usize>>,
  E: ParseError<I>,
  F: Parser<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser.parse(i) {
      Ok((remaining, result)) => {
        let index = input.offset(&remaining);
        let consumed = input.slice(..index);
        Ok((remaining, (consumed, result)))
      }
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn consumedc<I, O, E: ParseError<I>, F>(input: I, parser: F) -> IResult<I, (I, O), E>
where
  I: Clone + Offset + Slice<RangeTo<usize>>,
  E: ParseError<E>,
  F: Fn(I) -> IResult<I, O, E>,
{
  consumed(parser)(input)
}

/// transforms an error to failure
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::cut;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let mut parser = cut(alpha1);
///
/// assert_eq!(parser("abcd;"), Ok((";", "abcd")));
/// assert_eq!(parser("123;"), Err(Err::Failure(("123;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn cut<I, O, E: ParseError<I>, F>(mut parser: F) -> impl FnMut(I) -> IResult<I, O, E>
where
  F: Parser<I, O, E>,
{
  move |input: I| match parser.parse(input) {
    Err(Err::Error(e)) => Err(Err::Failure(e)),
    rest => rest,
  }
}

#[doc(hidden)]
pub fn cutc<I, O, E: ParseError<I>, F>(input: I, parser: F) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  cut(parser)(input)
}

/// automatically converts the child parser's result to another type
///
/// it will be able to convert the output value and the error value
/// as long as the `Into` implementations are available
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// use nom::combinator::into;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
///  fn parser1(i: &str) -> IResult<&str, &str> {
///    alpha1(i)
///  }
///
///  let mut parser2 = into(parser1);
///
/// // the parser converts the &str output of the child parser into a Vec<u8>
/// let bytes: IResult<&str, Vec<u8>> = parser2("abcd");
/// assert_eq!(bytes, Ok(("", vec![97, 98, 99, 100])));
/// # }
/// ```
pub fn into<I, O1, O2, E1, E2, F>(mut parser: F) -> impl FnMut(I) -> IResult<I, O2, E2>
where
  O1: Into<O2>,
  E1: Into<E2>,
  E1: ParseError<I>,
  E2: ParseError<I>,
  F: Parser<I, O1, E1>,
{
  //map(parser, Into::into)
  move |input: I| match parser.parse(input) {
    Ok((i, o)) => Ok((i, o.into())),
    Err(Err::Error(e)) => Err(Err::Error(e.into())),
    Err(Err::Failure(e)) => Err(Err::Failure(e.into())),
    Err(Err::Incomplete(e)) => Err(Err::Incomplete(e)),
  }
}

#[doc(hidden)]
pub fn intoc<I, O1, O2, E1, E2, F>(input: I, parser: F) -> IResult<I, O2, E2>
where
  O1: Into<O2>,
  E1: Into<E2>,
  E1: ParseError<I>,
  E2: ParseError<I>,
  F: Parser<I, O1, E1>,
{
  into(parser)(input)
}

/// Creates an iterator from input data and a parser.
///
/// Call the iterator's [ParserIterator::finish] method to get the remaining input if successful,
/// or the error value if we encountered an error.
///
/// ```rust
/// use nom::{combinator::iterator, IResult, bytes::complete::tag, character::complete::alpha1, sequence::terminated};
/// use std::collections::HashMap;
///
/// let data = "abc|defg|hijkl|mnopqr|123";
/// let mut it = iterator(data, terminated(alpha1, tag("|")));
///
/// let parsed = it.map(|v| (v, v.len())).collect::<HashMap<_,_>>();
/// let res: IResult<_,_> = it.finish();
///
/// assert_eq!(parsed, [("abc", 3usize), ("defg", 4), ("hijkl", 5), ("mnopqr", 6)].iter().cloned().collect());
/// assert_eq!(res, Ok(("123", ())));
/// ```
pub fn iterator<Input, Output, Error, F>(input: Input, f: F) -> ParserIterator<Input, Error, F>
where
  F: Parser<Input, Output, Error>,
  Error: ParseError<Input>,
{
  ParserIterator {
    iterator: f,
    input,
    state: Some(State::Running),
  }
}

/// Main structure associated to the [iterator] function.
pub struct ParserIterator<I, E, F> {
  iterator: F,
  input: I,
  state: Option<State<E>>,
}

impl<I: Clone, E, F> ParserIterator<I, E, F> {
  /// Returns the remaining input if parsing was successful, or the error if we encountered an error.
  pub fn finish(mut self) -> IResult<I, (), E> {
    match self.state.take().unwrap() {
      State::Running | State::Done => Ok((self.input, ())),
      State::Failure(e) => Err(Err::Failure(e)),
      State::Incomplete(i) => Err(Err::Incomplete(i)),
    }
  }
}

impl<'a, Input, Output, Error, F> core::iter::Iterator for &'a mut ParserIterator<Input, Error, F>
where
  F: FnMut(Input) -> IResult<Input, Output, Error>,
  Input: Clone,
{
  type Item = Output;

  fn next(&mut self) -> Option<Self::Item> {
    if let State::Running = self.state.take().unwrap() {
      let input = self.input.clone();

      match (self.iterator)(input) {
        Ok((i, o)) => {
          self.input = i;
          self.state = Some(State::Running);
          Some(o)
        }
        Err(Err::Error(_)) => {
          self.state = Some(State::Done);
          None
        }
        Err(Err::Failure(e)) => {
          self.state = Some(State::Failure(e));
          None
        }
        Err(Err::Incomplete(i)) => {
          self.state = Some(State::Incomplete(i));
          None
        }
      }
    } else {
      None
    }
  }
}

enum State<E> {
  Running,
  Done,
  Failure(E),
  Incomplete(Needed),
}

/// a parser which always succeeds with given value without consuming any input.
///
/// It can be used for example as the last alternative in `alt` to
/// specify the default case.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::branch::alt;
/// use nom::combinator::{success, value};
/// use nom::character::complete::char;
/// # fn main() {
///
/// let mut parser = success::<_,_,(_,ErrorKind)>(10);
/// assert_eq!(parser("xyz"), Ok(("xyz", 10)));
///
/// let mut sign = alt((value(-1, char('-')), value(1, char('+')), success::<_,_,(_,ErrorKind)>(1)));
/// assert_eq!(sign("+10"), Ok(("10", 1)));
/// assert_eq!(sign("-10"), Ok(("10", -1)));
/// assert_eq!(sign("10"), Ok(("10", 1)));
/// # }
/// ```
pub fn success<I, O: Clone, E: ParseError<I>>(val: O) -> impl Fn(I) -> IResult<I, O, E> {
  move |input: I| Ok((input, val.clone()))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::bytes::complete::take;
  use crate::error::ParseError;
  use crate::internal::{Err, IResult, Needed};
  use crate::number::complete::u8;

  macro_rules! assert_parse(
    ($left: expr, $right: expr) => {
      let res: $crate::IResult<_, _, (_, ErrorKind)> = $left;
      assert_eq!(res, $right);
    };
  );

  /*#[test]
  fn t1() {
    let v1:Vec<u8> = vec![1,2,3];
    let v2:Vec<u8> = vec![4,5,6];
    let d = Ok((&v1[..], &v2[..]));
    let res = d.flat_map(print);
    assert_eq!(res, Ok((&v2[..], ())));
  }*/

  #[test]
  fn eof_on_slices() {
    let not_over: &[u8] = &b"Hello, world!"[..];
    let is_over: &[u8] = &b""[..];

    let res_not_over = eof(not_over);
    assert_parse!(
      res_not_over,
      Err(Err::Error(error_position!(not_over, ErrorKind::Eof)))
    );

    let res_over = eof(is_over);
    assert_parse!(res_over, Ok((is_over, is_over)));
  }

  #[test]
  fn eof_on_strs() {
    let not_over: &str = "Hello, world!";
    let is_over: &str = "";

    let res_not_over = eof(not_over);
    assert_parse!(
      res_not_over,
      Err(Err::Error(error_position!(not_over, ErrorKind::Eof)))
    );

    let res_over = eof(is_over);
    assert_parse!(res_over, Ok((is_over, is_over)));
  }

  /*
  #[test]
  fn end_of_input() {
      let not_over = &b"Hello, world!"[..];
      let is_over = &b""[..];
      named!(eof_test, eof!());

      let res_not_over = eof_test(not_over);
      assert_eq!(res_not_over, Err(Err::Error(error_position!(not_over, ErrorKind::Eof))));

      let res_over = eof_test(is_over);
      assert_eq!(res_over, Ok((is_over, is_over)));
  }
  */

  #[test]
  fn rest_on_slices() {
    let input: &[u8] = &b"Hello, world!"[..];
    let empty: &[u8] = &b""[..];
    assert_parse!(rest(input), Ok((empty, input)));
  }

  #[test]
  fn rest_on_strs() {
    let input: &str = "Hello, world!";
    let empty: &str = "";
    assert_parse!(rest(input), Ok((empty, input)));
  }

  #[test]
  fn rest_len_on_slices() {
    let input: &[u8] = &b"Hello, world!"[..];
    assert_parse!(rest_len(input), Ok((input, input.len())));
  }

  use crate::lib::std::convert::From;
  impl From<u32> for CustomError {
    fn from(_: u32) -> Self {
      CustomError
    }
  }

  impl<I> ParseError<I> for CustomError {
    fn from_error_kind(_: I, _: ErrorKind) -> Self {
      CustomError
    }

    fn append(_: I, _: ErrorKind, _: CustomError) -> Self {
      CustomError
    }
  }

  struct CustomError;
  #[allow(dead_code)]
  fn custom_error(input: &[u8]) -> IResult<&[u8], &[u8], CustomError> {
    //fix_error!(input, CustomError, alphanumeric)
    crate::character::streaming::alphanumeric1(input)
  }

  #[test]
  fn test_flat_map() {
    let input: &[u8] = &[3, 100, 101, 102, 103, 104][..];
    assert_parse!(
      flat_map(u8, take)(input),
      Ok((&[103, 104][..], &[100, 101, 102][..]))
    );
  }

  #[test]
  fn test_map_opt() {
    let input: &[u8] = &[50][..];
    assert_parse!(
      map_opt(u8, |u| if u < 20 { Some(u) } else { None })(input),
      Err(Err::Error((&[50][..], ErrorKind::MapOpt)))
    );
    assert_parse!(
      map_opt(u8, |u| if u > 20 { Some(u) } else { None })(input),
      Ok((&[][..], 50))
    );
  }

  #[test]
  fn test_map_parser() {
    let input: &[u8] = &[100, 101, 102, 103, 104][..];
    assert_parse!(
      map_parser(take(4usize), take(2usize))(input),
      Ok((&[104][..], &[100, 101][..]))
    );
  }

  #[test]
  fn test_all_consuming() {
    let input: &[u8] = &[100, 101, 102][..];
    assert_parse!(
      all_consuming(take(2usize))(input),
      Err(Err::Error((&[102][..], ErrorKind::Eof)))
    );
    assert_parse!(
      all_consuming(take(3usize))(input),
      Ok((&[][..], &[100, 101, 102][..]))
    );
  }

  #[test]
  #[allow(unused)]
  fn test_verify_ref() {
    use crate::bytes::complete::take;

    let mut parser1 = verify(take(3u8), |s: &[u8]| s == &b"abc"[..]);

    assert_eq!(parser1(&b"abcd"[..]), Ok((&b"d"[..], &b"abc"[..])));
    assert_eq!(
      parser1(&b"defg"[..]),
      Err(Err::Error((&b"defg"[..], ErrorKind::Verify)))
    );

    fn parser2(i: &[u8]) -> IResult<&[u8], u32> {
      verify(crate::number::streaming::be_u32, |val: &u32| *val < 3)(i)
    }
  }

  #[test]
  #[cfg(feature = "alloc")]
  fn test_verify_alloc() {
    use crate::bytes::complete::take;
    let mut parser1 = verify(map(take(3u8), |s: &[u8]| s.to_vec()), |s: &[u8]| {
      s == &b"abc"[..]
    });

    assert_eq!(parser1(&b"abcd"[..]), Ok((&b"d"[..], (&b"abc").to_vec())));
    assert_eq!(
      parser1(&b"defg"[..]),
      Err(Err::Error((&b"defg"[..], ErrorKind::Verify)))
    );
  }

  #[test]
  #[cfg(feature = "std")]
  fn test_into() {
    use crate::bytes::complete::take;
    use crate::{
      error::{Error, ParseError},
      Err,
    };

    let mut parser = into(take::<_, _, Error<_>>(3u8));
    let result: IResult<&[u8], Vec<u8>> = parser(&b"abcdefg"[..]);

    assert_eq!(result, Ok((&b"defg"[..], vec![97, 98, 99])));
  }
}
