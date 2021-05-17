//! combinators applying their child parser multiple times

#[macro_use]
mod macros;

use crate::internal::{Err, IResult, Needed};
use crate::error::ParseError;
use crate::traits::{InputLength, InputTake, ToUsize};
#[cfg(feature = "alloc")]
use crate::lib::std::vec::Vec;
use crate::error::ErrorKind;

/// Repeats the embedded parser until it fails
/// and returns the results in a `Vec`.
///
/// # Arguments
/// * `f` The parser to apply.
///
/// *Note*: if the parser passed to `many0` accepts empty inputs
/// (like `alpha0` or `digit0`), `many0` will return an error,
/// to prevent going into an infinite loop
///
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many0;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   many0(tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Ok(("123123", vec![])));
/// assert_eq!(parser(""), Ok(("", vec![])));
/// ```
#[cfg(feature = "alloc")]
pub fn many0<I, O, E, F>(f: F) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut acc = crate::lib::std::vec::Vec::with_capacity(4);
    let mut i = i.clone();
    loop {
      match f(i.clone()) {
        Err(Err::Error(_)) => return Ok((i, acc)),
        Err(e) => return Err(e),
        Ok((i1, o)) => {
          if i1 == i {
            return Err(Err::Error(E::from_error_kind(i, ErrorKind::Many0)));
          }

          i = i1;
          acc.push(o);
        }
      }
    }
  }
}
// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn many0c<I, O, E, F>(input: I, f: F) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  many0(f)(input)
}

/// Runs the embedded parser until it fails and
/// returns the results in a `Vec`. Fails if
/// the embedded parser does not produce at least
/// one result.
///
/// # Arguments
/// * `f` The parser to apply.
///
/// *Note*: if the parser passed to `many1` accepts empty inputs
/// (like `alpha0` or `digit0`), `many1` will return an error,
/// to prevent going into an infinite loop
///
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many1;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   many1(tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Err(Err::Error(("123123", ErrorKind::Tag))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// ```
#[cfg(feature = "alloc")]
pub fn many1<I, O, E, F>(f: F) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut i = i.clone();
    match f(i.clone()) {
      Err(Err::Error(err)) => return Err(Err::Error(E::append(i, ErrorKind::Many1, err))),
      Err(e) => return Err(e),
      Ok((i1, o)) => {
        let mut acc = crate::lib::std::vec::Vec::with_capacity(4);
        acc.push(o);
        i = i1;

        loop {
          match f(i.clone()) {
            Err(Err::Error(_)) => return Ok((i, acc)),
            Err(e) => return Err(e),
            Ok((i1, o)) => {
              if i1 == i {
                return Err(Err::Error(E::from_error_kind(i, ErrorKind::Many1)));
              }

              i = i1;
              acc.push(o);
            }
          }
        }
      }
    }
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn many1c<I, O, E, F>(input: I, f: F) -> IResult<I, Vec<O>, E>
where
  I: Clone + Copy + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  many1(f)(input)
}

/// Applies the parser `f` until the parser `g` produces
/// a result. Returns a pair consisting of the results of
/// `f` in a `Vec` and the result of `g`.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many_till;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, (Vec<&str>, &str)> {
///   many_till(tag("abc"), tag("end"))(s)
/// };
///
/// assert_eq!(parser("abcabcend"), Ok(("", (vec!["abc", "abc"], "end"))));
/// assert_eq!(parser("abc123end"), Err(Err::Error(("123end", ErrorKind::Tag))));
/// assert_eq!(parser("123123end"), Err(Err::Error(("123123end", ErrorKind::Tag))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("abcendefg"), Ok(("efg", (vec!["abc"], "end"))));
/// ```
#[cfg(feature = "alloc")]
pub fn many_till<I, O, P, E, F, G>(f: F, g: G) -> impl Fn(I) -> IResult<I, (Vec<O>, P), E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, P, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut res = crate::lib::std::vec::Vec::new();
    let mut i = i.clone();
    loop {
      match g(i.clone()) {
        Ok((i1, o)) => return Ok((i1, (res, o))),
        Err(Err::Error(_)) => {
          match f(i.clone()) {
            Err(Err::Error(err)) =>
              return Err(Err::Error(E::append(i, ErrorKind::ManyTill, err))),
            Err(e) => return Err(e),
            Ok((i1, o)) => {
              // loop trip must always consume (otherwise infinite loops)
              if i1 == i {
                return Err(Err::Error(E::from_error_kind(i1, ErrorKind::ManyTill)));
              }

              res.push(o);
              i = i1;
            }
          }
        },
        Err(e) => return Err(e),
      }
    }
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn many_tillc<I, O, P, E, F, G>(i: I, f: F, g: G) -> IResult<I, (Vec<O>, P), E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, P, E>,
  E: ParseError<I>,
{
  many_till(f, g)(i)
}

/// Alternates between two parsers to produce
/// a list of elements.
/// # Arguments
/// * `sep` Parses the separator between list elements.
/// * `f` Parses the elements of the list.
///
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::separated_list;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   separated_list(tag("|"), tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abc|abc|abc"), Ok(("", vec!["abc", "abc", "abc"])));
/// assert_eq!(parser("abc123abc"), Ok(("123abc", vec!["abc"])));
/// assert_eq!(parser("abc|def"), Ok(("|def", vec!["abc"])));
/// assert_eq!(parser(""), Ok(("", vec![])));
/// assert_eq!(parser("def|abc"), Ok(("def|abc", vec![])));
/// ```
#[cfg(feature = "alloc")]
pub fn separated_list<I, O, O2, E, F, G>(sep: G, f: F) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut res = Vec::new();
    let mut i = i.clone();

    match f(i.clone()) {
      Err(Err::Error(_)) => return Ok((i, res)),
      Err(e) => return Err(e),
      Ok((i1, o)) => {
        if i1 == i {
          return Err(Err::Error(E::from_error_kind(i1, ErrorKind::SeparatedList)));
        }

        res.push(o);
        i = i1;
      }
    }

    loop {
      match sep(i.clone()) {
        Err(Err::Error(_)) => return Ok((i, res)),
        Err(e) => return Err(e),
        Ok((i1, _)) => {
          if i1 == i {
            return Err(Err::Error(E::from_error_kind(i1, ErrorKind::SeparatedList)));
          }

          match f(i1.clone()) {
            Err(Err::Error(_)) => return Ok((i, res)),
            Err(e) => return Err(e),
            Ok((i2, o)) => {
              if i2 == i {
                return Err(Err::Error(E::from_error_kind(i2, ErrorKind::SeparatedList)));
              }

              res.push(o);
              i = i2;
            }
          }
        }
      }
    }
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn separated_listc<I, O, O2, E, F, G>(i: I, sep: G, f: F) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  E: ParseError<I>,
{
  separated_list(sep, f)(i)
}

/// Alternates between two parsers to produce
/// a list of elements. Fails if the element
/// parser does not produce at least one element.
/// # Arguments
/// * `sep` Parses the separator between list elements.
/// * `f` Parses the elements of the list.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::separated_nonempty_list;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   separated_nonempty_list(tag("|"), tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abc|abc|abc"), Ok(("", vec!["abc", "abc", "abc"])));
/// assert_eq!(parser("abc123abc"), Ok(("123abc", vec!["abc"])));
/// assert_eq!(parser("abc|def"), Ok(("|def", vec!["abc"])));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("def|abc"), Err(Err::Error(("def|abc", ErrorKind::Tag))));
/// ```
#[cfg(feature = "alloc")]
pub fn separated_nonempty_list<I, O, O2, E, F, G>(sep: G, f: F) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut res = Vec::new();
    let mut i = i.clone();

    // Parse the first element
    match f(i.clone()) {
      Err(e)=> return Err(e),
      Ok((i1, o)) => {
        if i1 == i {
          return Err(Err::Error(E::from_error_kind(i1, ErrorKind::SeparatedList)));
        }

        res.push(o);
        i = i1;
      }
    }

    loop {
      match sep(i.clone()) {
        Err(Err::Error(_)) => return Ok((i, res)),
        Err(e) => return Err(e),
        Ok((i1, _)) => {
          if i1 == i {
            return Err(Err::Error(E::from_error_kind(i1, ErrorKind::SeparatedList)));
          }

          match f(i1.clone()) {
            Err(Err::Error(_)) => return Ok((i, res)),
            Err(e) => return Err(e),
            Ok((i2, o)) => {
              if i2 == i {
                return Err(Err::Error(E::from_error_kind(i2, ErrorKind::SeparatedList)));
              }

              res.push(o);
              i = i2;
            }
          }
        }
      }
    }
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn separated_nonempty_listc<I, O, O2, E, F, G>(i: I, sep: G, f: F) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  E: ParseError<I>,
{
  separated_nonempty_list(sep, f)(i)
}

/// Repeats the embedded parser `n` times or until it fails
/// and returns the results in a `Vec`. Fails if the
/// embedded parser does not succeed at least `m` times.
/// # Arguments
/// * `m` The minimum number of iterations.
/// * `n` The maximum number of iterations.
/// * `f` The parser to apply.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many_m_n;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   many_m_n(0, 2, tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Ok(("123123", vec![])));
/// assert_eq!(parser(""), Ok(("", vec![])));
/// assert_eq!(parser("abcabcabc"), Ok(("abc", vec!["abc", "abc"])));
/// ```
#[cfg(feature = "alloc")]
pub fn many_m_n<I, O, E, F>(m: usize, n: usize, f: F) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut res = crate::lib::std::vec::Vec::with_capacity(m);
    let mut input = i.clone();
    let mut count: usize = 0;

    if n == 0 {
        return Ok((i, vec!()))
    }

    loop {
      let _i = input.clone();
      match f(_i) {
        Ok((i, o)) => {
          // do not allow parsers that do not consume input (causes infinite loops)
          if i == input {
            return Err(Err::Error(E::from_error_kind(input, ErrorKind::ManyMN)));
          }

          res.push(o);
          input = i;
          count += 1;

          if count == n {
            return Ok((input, res));
          }
        }
        Err(Err::Error(e)) => {
          if count < m {
            return Err(Err::Error(E::append(input, ErrorKind::ManyMN, e)));
          } else {
            return Ok((input, res));
          }
        }
        Err(e) => {
          return Err(e);
        }
      }
    }
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn many_m_nc<I, O, E, F>(i: I, m: usize, n: usize, f: F) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  many_m_n(m, n, f)(i)
}

/// Repeats the embedded parser until it fails
/// and returns the number of successful iterations.
/// # Arguments
/// * `f` The parser to apply.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many0_count;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, usize> {
///   many0_count(tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", 2)));
/// assert_eq!(parser("abc123"), Ok(("123", 1)));
/// assert_eq!(parser("123123"), Ok(("123123", 0)));
/// assert_eq!(parser(""), Ok(("", 0)));
/// ```
pub fn many0_count<I, O, E, F>(f: F) -> impl Fn(I) -> IResult<I, usize, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let mut input = i.clone();
    let mut count = 0;

    loop {
      let input_ = input.clone();
      match f(input_) {
        Ok((i, _)) => {
          //  loop trip must always consume (otherwise infinite loops)
          if i == input {
            return Err(Err::Error(E::from_error_kind(input, ErrorKind::Many0Count)));
          }

          input = i;
          count += 1;
        }

        Err(Err::Error(_)) => return Ok((input, count)),

        Err(e) => return Err(e),
      }
    }
  }
}

#[doc(hidden)]
pub fn many0_countc<I, O, E, F>(i: I, f: F) -> IResult<I, usize, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  many0_count(f)(i)
}

/// Repeats the embedded parser until it fails
/// and returns the number of successful iterations.
/// Fails if the embedded parser does not succeed
/// at least once.
/// # Arguments
/// * `f` The parser to apply.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::many1_count;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, usize> {
///   many1_count(tag("abc"))(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", 2)));
/// assert_eq!(parser("abc123"), Ok(("123", 1)));
/// assert_eq!(parser("123123"), Err(Err::Error(("123123", ErrorKind::Many1Count))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Many1Count))));
/// ```
pub fn many1_count<I, O, E, F>(f: F) -> impl Fn(I) -> IResult<I, usize, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let i_ = i.clone();
    match f(i_) {
      Err(Err::Error(_)) => Err(Err::Error(E::from_error_kind(i, ErrorKind::Many1Count))),
      Err(i) => Err(i),
      Ok((i1, _)) => {
        let mut count = 1;
        let mut input = i1;

        loop {
          let input_ = input.clone();
          match f(input_) {
            Err(Err::Error(_)) => return Ok((input, count)),
            Err(e) => return Err(e),
            Ok((i, _)) => {
              if i == input {
                return Err(Err::Error(E::from_error_kind(i, ErrorKind::Many1Count)));
              }

              count += 1;
              input = i;
            }
          }
        }
      }
    }
  }
}

#[doc(hidden)]
pub fn many1_countc<I, O, E, F>(i: I, f: F) -> IResult<I, usize, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  many1_count(f)(i)
}

/// Runs the embedded parser a specified number
/// of times. Returns the results in a `Vec`.
/// # Arguments
/// * `f` The parser to apply.
/// * `count` How often to apply the parser.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::count;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   count(tag("abc"), 2)(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// assert_eq!(parser("123123"), Err(Err::Error(("123123", ErrorKind::Tag))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("abcabcabc"), Ok(("abc", vec!["abc", "abc"])));
/// ```
#[cfg(feature = "alloc")]
pub fn count<I, O, E, F>(f: F, count: usize) -> impl Fn(I) -> IResult<I, Vec<O>, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I | {
    let mut input = i.clone();
    let mut res = crate::lib::std::vec::Vec::new();

    for _ in 0..count {
      let input_ = input.clone();
      match f(input_) {
        Ok((i, o)) => {
          res.push(o);
          input = i;
        }
        Err(Err::Error(e)) => {
          return Err(Err::Error(E::append(i, ErrorKind::Count, e)));
        }
        Err(e) => {
          return Err(e);
        }
      }
    }

    Ok((input, res))
  }
}

/// Applies a parser until it fails and accumulates
/// the results using a given function and initial value.
/// # Arguments
/// * `f` The parser to apply.
/// * `init` The initial value.
/// * `g` The function that combines a result of `f` with
///       the current accumulator.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::fold_many0;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   fold_many0(
///     tag("abc"),
///     Vec::new(),
///     |mut acc: Vec<_>, item| {
///       acc.push(item);
///       acc
///     }
///   )(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Ok(("123123", vec![])));
/// assert_eq!(parser(""), Ok(("", vec![])));
/// ```
pub fn fold_many0<I, O, E, F, G, R>(f: F, init: R, g: G) -> impl Fn(I) -> IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  move |i: I| {
    let mut res = init.clone();
    let mut input = i.clone();

    loop {
      let i_ = input.clone();
      match f(i_) {
        Ok((i, o)) => {
          // loop trip must always consume (otherwise infinite loops)
          if i == input {
            return Err(Err::Error(E::from_error_kind(input, ErrorKind::Many0)));
          }

          res = g(res, o);
          input = i;
        }
        Err(Err::Error(_)) => {
          return Ok((input, res));
        }
        Err(e) => {
          return Err(e);
        }
      }
    }
  }
}

#[doc(hidden)]
pub fn fold_many0c<I, O, E, F, G, R>(i: I, f: F, init: R, g: G) -> IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  fold_many0(f, init, g)(i)
}

/// Applies a parser until it fails and accumulates
/// the results using a given function and initial value.
/// Fails if the embedded parser does not succeed at least
/// once.
/// # Arguments
/// * `f` The parser to apply.
/// * `init` The initial value.
/// * `g` The function that combines a result of `f` with
///       the current accumulator.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::fold_many1;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   fold_many1(
///     tag("abc"),
///     Vec::new(),
///     |mut acc: Vec<_>, item| {
///       acc.push(item);
///       acc
///     }
///   )(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Err(Err::Error(("123123", ErrorKind::Many1))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Many1))));
/// ```
pub fn fold_many1<I, O, E, F, G, R>(f: F, init: R, g: G) -> impl Fn(I) -> IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  move |i: I| {
    let _i = i.clone();
    let init = init.clone();
    match f(_i) {
      Err(Err::Error(_)) => Err(Err::Error(E::from_error_kind(i, ErrorKind::Many1))),
      Err(e) => return Err(e),
      Ok((i1, o1)) => {
        let mut acc = g(init, o1);
        let mut input = i1;

        loop {
          let _input = input.clone();
          match f(_input) {
            Err(Err::Error(_)) => {
              break;
            }
            Err(e) => return Err(e),
            Ok((i, o)) => {
              if i == input {
                return Err(Err::Failure(E::from_error_kind(i, ErrorKind::Many1)));
              }

              acc = g(acc, o);
              input = i;
            }
          }
        }

        Ok((input, acc))
      }
    }
  }
}

#[doc(hidden)]
pub fn fold_many1c<I, O, E, F, G, R>(i: I, f: F, init: R, g: G) -> IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  fold_many1(f, init, g)(i)
}

/// Applies a parser `n` times or until it fails and accumulates
/// the results using a given function and initial value.
/// Fails if the embedded parser does not succeed at least `m`
/// times.
/// # Arguments
/// * `m` The minimum number of iterations.
/// * `n` The maximum number of iterations.
/// * `f` The parser to apply.
/// * `init` The initial value.
/// * `g` The function that combines a result of `f` with
///       the current accumulator.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// use nom::multi::fold_many_m_n;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &str) -> IResult<&str, Vec<&str>> {
///   fold_many_m_n(
///     0,
///     2,
///     tag("abc"),
///     Vec::new(),
///     |mut acc: Vec<_>, item| {
///       acc.push(item);
///       acc
///     }
///   )(s)
/// }
///
/// assert_eq!(parser("abcabc"), Ok(("", vec!["abc", "abc"])));
/// assert_eq!(parser("abc123"), Ok(("123", vec!["abc"])));
/// assert_eq!(parser("123123"), Ok(("123123", vec![])));
/// assert_eq!(parser(""), Ok(("", vec![])));
/// assert_eq!(parser("abcabcabc"), Ok(("abc", vec!["abc", "abc"])));
/// ```
pub fn fold_many_m_n<I, O, E, F, G, R>(m: usize, n: usize, f: F, init: R, g: G) -> impl Fn(I) ->IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  move |i: I| {
    let mut acc = init.clone();
    let mut input = i.clone();
    for count in 0..n {
      let _input = input.clone();
      match f(_input) {
        Ok((i, o)) => {
          // do not allow parsers that do not consume input (causes infinite loops)
          if i == input {
            return Err(Err::Error(E::from_error_kind(i, ErrorKind::ManyMN)));
          }

          acc = g(acc, o);
          input = i;
        }
        //FInputXMError: handle failure properly
        Err(Err::Error(_)) => if count < m {
          return Err(Err::Error(E::from_error_kind(i, ErrorKind::ManyMN)));
        } else {
          break;
        }
        Err(e) => return Err(e),
      }
    }

    Ok((input, acc))
  }
}

#[doc(hidden)]
pub fn fold_many_m_nc<I, O, E, F, G, R>(i: I, m: usize, n: usize, f: F, init: R, g: G) -> IResult<I, R, E>
where
  I: Clone + PartialEq,
  F: Fn(I) -> IResult<I, O, E>,
  G: Fn(R, O) -> R,
  E: ParseError<I>,
  R: Clone,
{
  fold_many_m_n(m, n, f, init, g)(i)
}

/// Gets a number from the parser and returns a
/// subslice of the input of that size.
/// If the parser returns Incomplete,
/// length_data will return an error.
/// # Arguments
/// * `f` The parser to apply.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// # use nom::Needed::Size;
/// use nom::number::complete::be_u16;
/// use nom::multi::length_data;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &[u8]) -> IResult<&[u8], &[u8]> {
///   length_data(be_u16)(s)
/// }
///
/// assert_eq!(parser(b"\x00\x03abcefg"), Ok((&b"efg"[..], &b"abc"[..])));
/// assert_eq!(parser(b"\x00\x03"), Err(Err::Incomplete(Size(3))));
/// ```
pub fn length_data<I, N, E, F>(f: F) -> impl Fn(I) -> IResult<I, I, E>
where
  I: Clone + InputLength + InputTake,
  N: Copy + ToUsize,
  F: Fn(I) -> IResult<I, N, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let (i, length) = f(i)?;

    let length: usize = length.to_usize();

    if i.input_len() < length {
      Err(Err::Incomplete(Needed::Size(length)))
    } else {
      Ok(i.take_split(length))
    }
  }
}

/// Gets a number from the first parser,
/// takes a subslice of the input of that size,
/// then applies the second parser on that subslice.
/// If the second parser returns Incomplete,
/// length_value will return an error.
/// # Arguments
/// * `f` The parser to apply.
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind, Needed, IResult};
/// # use nom::Needed::Size;
/// use nom::number::complete::be_u16;
/// use nom::multi::length_value;
/// use nom::bytes::complete::tag;
///
/// fn parser(s: &[u8]) -> IResult<&[u8], &[u8]> {
///   length_value(be_u16, tag("abc"))(s)
/// }
///
/// assert_eq!(parser(b"\x00\x03abcefg"), Ok((&b"efg"[..], &b"abc"[..])));
/// assert_eq!(parser(b"\x00\x03123123"), Err(Err::Error((&b"123"[..], ErrorKind::Tag))));
/// assert_eq!(parser(b"\x00\x03"), Err(Err::Incomplete(Size(3))));
/// ```
pub fn length_value<I, O, N, E, F, G>(f: F, g: G) -> impl Fn(I) -> IResult<I, O, E>
where
  I: Clone + InputLength + InputTake,
  N: Copy + ToUsize,
  F: Fn(I) -> IResult<I, N, E>,
  G: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  move |i: I| {
    let (i, length) = f(i)?;

    let length: usize = length.to_usize();

    if i.input_len() < length {
      Err(Err::Incomplete(Needed::Size(length)))
    } else {
      let (rest, i) = i.take_split(length);
      match g(i.clone()) {
        Err(Err::Incomplete(_)) =>
          Err(Err::Error(E::from_error_kind(i, ErrorKind::Complete))),
        Err(e) => Err(e),
        Ok((_, o)) => Ok((rest,o)),
      }
    }
  }
}

#[doc(hidden)]
pub fn length_valuec<I, O, N, E, F, G>(i: I, f: F, g: G) -> IResult<I, O, E>
where
  I: Clone + InputLength + InputTake,
  N: Copy + ToUsize,
  F: Fn(I) -> IResult<I, N, E>,
  G: Fn(I) -> IResult<I, O, E>,
  E: ParseError<I>,
{
  length_value(f, g)(i)
}
