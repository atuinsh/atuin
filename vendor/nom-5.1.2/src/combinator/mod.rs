//! general purpose combinators

#![allow(unused_imports)]

#[cfg(feature = "alloc")]
use crate::lib::std::boxed::Box;

#[cfg(feature = "std")]
use crate::lib::std::fmt::Debug;
use crate::internal::*;
use crate::error::ParseError;
use crate::traits::{AsChar, InputIter, InputLength, InputTakeAtPosition, ParseTo};
use crate::lib::std::ops::{Range, RangeFrom, RangeTo};
use crate::lib::std::borrow::Borrow;
use crate::traits::{Compare, CompareResult, Offset, Slice};
use crate::error::ErrorKind;
use crate::lib::std::mem::transmute;

#[macro_use]
mod macros;

/// Return the remaining input
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
  T: Slice<Range<usize>> + Slice<RangeFrom<usize>> + Slice<RangeTo<usize>>,
  T: InputLength,
{
  Ok((input.slice(input.input_len()..), input))
}

/// Return the length of the remaining input
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
  T: Slice<Range<usize>> + Slice<RangeFrom<usize>> + Slice<RangeTo<usize>>,
  T: InputLength,
{
  let len = input.input_len();
  Ok((input, len))
}

/// maps a function on the result of a parser
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::combinator::map;
/// # fn main() {
///
/// let parse = map(digit1, |s: &str| s.len());
///
/// // the parser will count how many characters were returned by digit1
/// assert_eq!(parse("123456"), Ok(("", 6)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parse("abc"), Err(Err::Error(("abc", ErrorKind::Digit))));
/// # }
/// ```
pub fn map<I, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> O2,
{
  move |input: I| {
    let (input, o1) = first(input)?;
    Ok((input, second(o1)))
  }
}

#[doc(hidden)]
pub fn mapc<I, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> O2,
{
  map(first, second)(input)
}

/// applies a function returning a Result over the result of a parser
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::combinator::map_res;
/// # fn main() {
///
/// let parse = map_res(digit1, |s: &str| s.parse::<u8>());
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
pub fn map_res<I: Clone, O1, O2, E: ParseError<I>, E2, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Result<O2, E2>,
{
  move |input: I| {
    let i = input.clone();
    let (input, o1) = first(input)?;
    match second(o1) {
      Ok(o2) => Ok((input, o2)),
      Err(_) => Err(Err::Error(E::from_error_kind(i, ErrorKind::MapRes))),
    }
  }
}

#[doc(hidden)]
pub fn map_resc<I: Clone, O1, O2, E: ParseError<I>, E2, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Result<O2, E2>,
{
  map_res(first, second)(input)
}

/// applies a function returning an Option over the result of a parser
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::combinator::map_opt;
/// # fn main() {
///
/// let parse = map_opt(digit1, |s: &str| s.parse::<u8>().ok());
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
pub fn map_opt<I: Clone, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Option<O2>,
{
  move |input: I| {
    let i = input.clone();
    let (input, o1) = first(input)?;
    match second(o1) {
      Some(o2) => Ok((input, o2)),
      None => Err(Err::Error(E::from_error_kind(i, ErrorKind::MapOpt))),
    }
  }
}

#[doc(hidden)]
pub fn map_optc<I: Clone, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> Option<O2>,
{
  map_opt(first, second)(input)
}

/// applies a parser over the result of another one
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// use nom::bytes::complete::take;
/// use nom::combinator::map_parser;
/// # fn main() {
///
/// let parse = map_parser(take(5u8), digit1);
///
/// assert_eq!(parse("12345"), Ok(("", "12345")));
/// assert_eq!(parse("123ab"), Ok(("", "123")));
/// assert_eq!(parse("123"), Err(Err::Error(("123", ErrorKind::Eof))));
/// # }
/// ```
pub fn map_parser<I: Clone, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> IResult<O1, O2, E>,
  O1: InputLength,
{
  move |input: I| {
    let (input, o1) = first(input)?;
    let (_, o2) = second(o1)?;
    Ok((input, o2))
  }
}

#[doc(hidden)]
pub fn map_parserc<I: Clone, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> IResult<O1, O2, E>,
  O1: InputLength,
{
  map_parser(first, second)(input)
}

/// creates a new parser from the output of the first parser, then apply that parser over the rest of the input
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::bytes::complete::take;
/// use nom::number::complete::be_u8;
/// use nom::combinator::flat_map;
/// # fn main() {
///
/// let parse = flat_map(be_u8, take);
///
/// assert_eq!(parse(&[2, 0, 1, 2][..]), Ok((&[2][..], &[0, 1][..])));
/// assert_eq!(parse(&[4, 0, 1, 2][..]), Err(Err::Error((&[0, 1, 2][..], ErrorKind::Eof))));
/// # }
/// ```
pub fn flat_map<I, O1, O2, E: ParseError<I>, F, G, H>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(O1) -> H,
  H: Fn(I) -> IResult<I, O2, E>
{
  move |input: I| {
    let (input, o1) = first(input)?;
    second(o1)(input)
  }
}

/// optional parser: will return None if not successful
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
pub fn opt<I:Clone, O, E: ParseError<I>, F>(f: F) -> impl Fn(I) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f(input) {
      Ok((i, o)) => Ok((i, Some(o))),
      Err(Err::Error(_)) => Ok((i, None)),
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn optc<I:Clone, O, E: ParseError<I>, F>(input: I, f: F) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  opt(f)(input)
}

/// calls the parser if the condition is met
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
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
/// assert_eq!(parser(true, "123;"), Err(Err::Error(("123;", ErrorKind::Alpha))));
/// assert_eq!(parser(false, "123;"), Ok(("123;", None)));
/// # }
/// ```
pub fn cond<I:Clone, O, E: ParseError<I>, F>(b: bool, f: F) -> impl Fn(I) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    if b {
      match f(input) {
        Ok((i, o)) => Ok((i, Some(o))),
        Err(e) => Err(e),
      }
    } else {
      Ok((input, None))
    }
  }
}

#[doc(hidden)]
pub fn condc<I:Clone, O, E: ParseError<I>, F>(input: I, b: bool, f: F) -> IResult<I, Option<O>, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  cond(b, f)(input)
}

/// tries to apply its parser without consuming the input
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::peek;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let parser = peek(alpha1);
///
/// assert_eq!(parser("abcd;"), Ok(("abcd;", "abcd")));
/// assert_eq!(parser("123;"), Err(Err::Error(("123;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn peek<I:Clone, O, E: ParseError<I>, F>(f: F) -> impl Fn(I) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f(input) {
      Ok((_, o)) => Ok((i, o)),
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn peekc<I:Clone, O, E: ParseError<I>, F>(input: I, f: F) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  peek(f)(input)
}

/// transforms Incomplete into Error
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::bytes::streaming::take;
/// use nom::combinator::complete;
/// # fn main() {
///
/// let parser = complete(take(5u8));
///
/// assert_eq!(parser("abcdefg"), Ok(("fg", "abcde")));
/// assert_eq!(parser("abcd"), Err(Err::Error(("abcd", ErrorKind::Complete))));
/// # }
/// ```
pub fn complete<I: Clone, O, E: ParseError<I>, F>(f: F) -> impl Fn(I) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match f(input) {
      Err(Err::Incomplete(_)) => {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::Complete)))
      },
      rest => rest
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

/// succeeds if all the input has been consumed by its child parser
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::all_consuming;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let parser = all_consuming(alpha1);
///
/// assert_eq!(parser("abcd"), Ok(("", "abcd")));
/// assert_eq!(parser("abcd;"),Err(Err::Error((";", ErrorKind::Eof))));
/// assert_eq!(parser("123abcd;"),Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn all_consuming<I, O, E: ParseError<I>, F>(f: F) -> impl Fn(I) -> IResult<I, O, E>
where
  I: InputLength,
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let (input, res) = f(input)?;
    if input.input_len() == 0 {
      Ok((input, res))
    } else {
      Err(Err::Error(E::from_error_kind(input, ErrorKind::Eof)))
    }
  }
}

/// returns the result of the child parser if it satisfies a verification function
///
/// the verification function takes as argument a reference to the output of the
/// parser
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::verify;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let parser = verify(alpha1, |s: &str| s.len() == 4);
///
/// assert_eq!(parser("abcd"), Ok(("", "abcd")));
/// assert_eq!(parser("abcde"), Err(Err::Error(("abcde", ErrorKind::Verify))));
/// assert_eq!(parser("123abcd;"),Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn verify<I: Clone, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(&O2) -> bool,
  O1: Borrow<O2>,
  O2: ?Sized,
{
  move |input: I| {
    let i = input.clone();
    let (input, o) = first(input)?;

    if second(o.borrow()) {
      Ok((input, o))
    } else {
      Err(Err::Error(E::from_error_kind(i, ErrorKind::Verify)))
    }
  }
}

#[doc(hidden)]
pub fn verifyc<I: Clone, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(&O2) -> bool,
  O1: Borrow<O2>,
  O2: ?Sized,
{
  verify(first, second)(input)
}

/// returns the provided value if the child parser succeeds
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::value;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let parser = value(1234, alpha1);
///
/// assert_eq!(parser("abcd"), Ok(("", 1234)));
/// assert_eq!(parser("123abcd;"), Err(Err::Error(("123abcd;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn value<I, O1: Clone, O2, E: ParseError<I>, F>(val: O1, parser: F) -> impl Fn(I) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O2, E>,
{
  move |input: I| {
    parser(input).map(|(i, _)| (i, val.clone()))
  }
}

#[doc(hidden)]
pub fn valuec<I, O1: Clone, O2, E: ParseError<I>, F>(input: I, val: O1, parser: F) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O2, E>,
{
  value(val, parser)(input)
}

/// succeeds if the child parser returns an error
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::not;
/// use nom::character::complete::alpha1;
/// # fn main() {
///
/// let parser = not(alpha1);
///
/// assert_eq!(parser("123"), Ok(("123", ())));
/// assert_eq!(parser("abcd"), Err(Err::Error(("abcd", ErrorKind::Not))));
/// # }
/// ```
pub fn not<I: Clone, O, E: ParseError<I>, F>(parser: F) -> impl Fn(I) -> IResult<I, (), E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser(input) {
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

/// if the child parser was successful, return the consumed input as produced value
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::combinator::recognize;
/// use nom::character::complete::{char, alpha1};
/// use nom::sequence::separated_pair;
/// # fn main() {
///
/// let parser = recognize(separated_pair(alpha1, char(','), alpha1));
///
/// assert_eq!(parser("abcd,efgh"), Ok(("", "abcd,efgh")));
/// assert_eq!(parser("abcd;"),Err(Err::Error((";", ErrorKind::Char))));
/// # }
/// ```
pub fn recognize<I: Clone + Offset + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(parser: F) -> impl Fn(I) -> IResult<I, I, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser(i) {
      Ok((i, _)) => {
        let index = input.offset(&i);
        Ok((i, input.slice(..index)))
      },
      Err(e) => Err(e),
    }
  }
}

#[doc(hidden)]
pub fn recognizec<I: Clone + Offset + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(input: I, parser: F) -> IResult<I, I, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  recognize(parser)(input)
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
/// let parser = cut(alpha1);
///
/// assert_eq!(parser("abcd;"), Ok((";", "abcd")));
/// assert_eq!(parser("123;"), Err(Err::Failure(("123;", ErrorKind::Alpha))));
/// # }
/// ```
pub fn cut<I: Clone + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(parser: F) -> impl Fn(I) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  move |input: I| {
    let i = input.clone();
    match parser(i) {
      Err(Err::Error(e)) => Err(Err::Failure(e)),
      rest => rest,
    }
  }
}

#[doc(hidden)]
pub fn cutc<I: Clone + Slice<RangeTo<usize>>, O, E: ParseError<I>, F>(input: I, parser: F) -> IResult<I, O, E>
where
  F: Fn(I) -> IResult<I, O, E>,
{
  cut(parser)(input)
}

/// creates an iterator from input data and a parser
///
/// call the iterator's [finish] method to get the remaining input if successful,
/// or the error value if we encountered an error
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
  F: Fn(Input) -> IResult<Input, Output, Error>,
  Error: ParseError<Input> {

    ParserIterator {
      iterator: f,
      input,
      state: State::Running,
    }
}

/// main structure associated to the [iterator] function
pub struct ParserIterator<I, E, F> {
  iterator: F,
  input: I,
  state: State<E>,
}

impl<I: Clone, E: Clone, F> ParserIterator<I, E, F> {
  /// returns the remaining input if parsing was successful, or the error if we encountered an error
  pub fn finish(self) -> IResult<I, (), E> {
    match &self.state {
      State::Running | State::Done => Ok((self.input.clone(), ())),
      State::Failure(e) => Err(Err::Failure(e.clone())),
      State::Incomplete(i) => Err(Err::Incomplete(i.clone())),
    }
  }
}

impl<'a, Input ,Output ,Error, F> core::iter::Iterator for &'a mut ParserIterator<Input, Error, F>
    where
    F: Fn(Input) -> IResult<Input, Output, Error>,
    Input: Clone
{
  type Item = Output;

  fn next(&mut self) -> Option<Self::Item> {
    if let State::Running = self.state {
      let input = self.input.clone();

      match (self.iterator)(input) {
        Ok((i, o)) => {
          self.input = i;
          Some(o)
        },
        Err(Err::Error(_)) => {
          self.state = State::Done;
          None
        },
        Err(Err::Failure(e)) => {
          self.state = State::Failure(e);
          None
        },
        Err(Err::Incomplete(i)) => {
          self.state = State::Incomplete(i);
          None
        },
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


#[cfg(test)]
mod tests {
  use super::*;
  use crate::internal::{Err, IResult, Needed};
  use crate::error::ParseError;
  use crate::bytes::complete::take;
  use crate::number::complete::be_u8;

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
      assert_parse!(flat_map(be_u8, take)(input), Ok((&[103, 104][..], &[100, 101, 102][..])));
  }

  #[test]
  fn test_map_opt() {
      let input: &[u8] = &[50][..];
      assert_parse!(map_opt(be_u8, |u| if u < 20 {Some(u)} else {None})(input), Err(Err::Error((&[50][..], ErrorKind::MapOpt))));
      assert_parse!(map_opt(be_u8, |u| if u > 20 {Some(u)} else {None})(input), Ok((&[][..], 50)));
  }

  #[test]
  fn test_map_parser() {
      let input: &[u8] = &[100, 101, 102, 103, 104][..];
      assert_parse!(map_parser(take(4usize), take(2usize))(input), Ok((&[104][..], &[100, 101][..])));
  }

  #[test]
  fn test_all_consuming() {
      let input: &[u8] = &[100, 101, 102][..];
      assert_parse!(all_consuming(take(2usize))(input), Err(Err::Error((&[102][..], ErrorKind::Eof))));
      assert_parse!(all_consuming(take(3usize))(input), Ok((&[][..], &[100, 101, 102][..])));
  }

  #[test]
  #[allow(unused)]
  fn test_verify_ref() {
    use crate::bytes::complete::take;

    let parser1 = verify(take(3u8), |s: &[u8]| s == &b"abc"[..]);

    assert_eq!(parser1(&b"abcd"[..]), Ok((&b"d"[..], &b"abc"[..])));
    assert_eq!(parser1(&b"defg"[..]), Err(Err::Error((&b"defg"[..], ErrorKind::Verify))));

    fn parser2(i: &[u8]) -> IResult<&[u8], u32> {
      verify(crate::number::streaming::be_u32, |val: &u32| *val < 3)(i)
    }
  }

  #[test]
  #[cfg(feature = "alloc")]
  fn test_verify_alloc() {
    use crate::bytes::complete::take;
    let parser1 = verify(map(take(3u8), |s: &[u8]| s.to_vec()), |s: &[u8]| s == &b"abc"[..]);

    assert_eq!(parser1(&b"abcd"[..]), Ok((&b"d"[..], (&b"abc").to_vec())));
    assert_eq!(parser1(&b"defg"[..]), Err(Err::Error((&b"defg"[..], ErrorKind::Verify))));
  }
}
