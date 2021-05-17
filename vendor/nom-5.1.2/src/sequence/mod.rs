//! combinators applying parsers in sequence

#[macro_use]
mod macros;

use crate::internal::IResult;
use crate::error::ParseError;

/// Gets an object from the first parser,
/// then gets another object from the second parser.
///
/// # Arguments
/// * `first` The first parser to apply.
/// * `second` The second parser to apply.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed};
/// # use nom::Needed::Size;
/// use nom::sequence::pair;
/// use nom::bytes::complete::tag;
///
/// let parser = pair(tag("abc"), tag("efg"));
///
/// assert_eq!(parser("abcefg"), Ok(("", ("abc", "efg"))));
/// assert_eq!(parser("abcefghij"), Ok(("hij", ("abc", "efg"))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// ```
pub fn pair<I, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, (O1, O2), E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  move |input: I| {
    let (input, o1) = first(input)?;
    second(input).map(|(i, o2)| (i, (o1, o2)))
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
pub fn pairc<I, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, (O1, O2), E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  pair(first, second)(input)
}

/// Matches an object from the first parser and discards it,
/// then gets an object from the second parser.
///
/// # Arguments
/// * `first` The opening parser.
/// * `second` The second parser to get object.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed};
/// # use nom::Needed::Size;
/// use nom::sequence::preceded;
/// use nom::bytes::complete::tag;
///
/// let parser = preceded(tag("abc"), tag("efg"));
///
/// assert_eq!(parser("abcefg"), Ok(("", "efg")));
/// assert_eq!(parser("abcefghij"), Ok(("hij", "efg")));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// ```
pub fn preceded<I, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  move |input: I| {
    let (input, _) = first(input)?;
    second(input)
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
pub fn precededc<I, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  preceded(first, second)(input)
}

/// Gets an object from the first parser,
/// then matches an object from the second parser and discards it.
///
/// # Arguments
/// * `first` The first parser to apply.
/// * `second` The second parser to match an object.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed};
/// # use nom::Needed::Size;
/// use nom::sequence::terminated;
/// use nom::bytes::complete::tag;
///
/// let parser = terminated(tag("abc"), tag("efg"));
///
/// assert_eq!(parser("abcefg"), Ok(("", "abc")));
/// assert_eq!(parser("abcefghij"), Ok(("hij", "abc")));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// ```
pub fn terminated<I, O1, O2, E: ParseError<I>, F, G>(first: F, second: G) -> impl Fn(I) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  move |input: I| {
    let (input, o1) = first(input)?;
    second(input).map(|(i, _)| (i, o1))
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
pub fn terminatedc<I, O1, O2, E: ParseError<I>, F, G>(input: I, first: F, second: G) -> IResult<I, O1, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
{
  terminated(first, second)(input)
}

/// Gets an object from the first parser,
/// then matches an object from the sep_parser and discards it,
/// then gets another object from the second parser.
///
/// # Arguments
/// * `first` The first parser to apply.
/// * `sep` The separator parser to apply.
/// * `second` The second parser to apply.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed};
/// # use nom::Needed::Size;
/// use nom::sequence::separated_pair;
/// use nom::bytes::complete::tag;
///
/// let parser = separated_pair(tag("abc"), tag("|"), tag("efg"));
///
/// assert_eq!(parser("abc|efg"), Ok(("", ("abc", "efg"))));
/// assert_eq!(parser("abc|efghij"), Ok(("hij", ("abc", "efg"))));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// ```
pub fn separated_pair<I, O1, O2, O3, E: ParseError<I>, F, G, H>(first: F, sep: G, second: H) -> impl Fn(I) -> IResult<I, (O1, O3), E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  H: Fn(I) -> IResult<I, O3, E>,
{
  move |input: I| {
    let (input, o1) = first(input)?;
    let (input, _) = sep(input)?;
    second(input).map(|(i, o2)| (i, (o1, o2)))
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
pub fn separated_pairc<I, O1, O2, O3, E: ParseError<I>, F, G, H>(input: I, first: F, sep: G, second: H) -> IResult<I, (O1, O3), E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  H: Fn(I) -> IResult<I, O3, E>,
{
  separated_pair(first, sep, second)(input)
}

/// Matches an object from the first parser,
/// then gets an object from the sep_parser,
/// then matches another object from the second parser.
///
/// # Arguments
/// * `first` The first parser to apply.
/// * `sep` The separator parser to apply.
/// * `second` The second parser to apply.
/// ```rust
/// # use nom::{Err, error::ErrorKind, Needed};
/// # use nom::Needed::Size;
/// use nom::sequence::delimited;
/// use nom::bytes::complete::tag;
///
/// let parser = delimited(tag("abc"), tag("|"), tag("efg"));
///
/// assert_eq!(parser("abc|efg"), Ok(("", "|")));
/// assert_eq!(parser("abc|efghij"), Ok(("hij", "|")));
/// assert_eq!(parser(""), Err(Err::Error(("", ErrorKind::Tag))));
/// assert_eq!(parser("123"), Err(Err::Error(("123", ErrorKind::Tag))));
/// ```
pub fn delimited<I, O1, O2, O3, E: ParseError<I>, F, G, H>(first: F, sep: G, second: H) -> impl Fn(I) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  H: Fn(I) -> IResult<I, O3, E>,
{
  move |input: I| {
    let (input, _) = first(input)?;
    let (input, o2) = sep(input)?;
    second(input).map(|(i, _)| (i, o2))
  }
}

// this implementation is used for type inference issues in macros
#[doc(hidden)]
pub fn delimitedc<I, O1, O2, O3, E: ParseError<I>, F, G, H>(input: I, first: F, sep: G, second: H) -> IResult<I, O2, E>
where
  F: Fn(I) -> IResult<I, O1, E>,
  G: Fn(I) -> IResult<I, O2, E>,
  H: Fn(I) -> IResult<I, O3, E>,
{
  delimited(first, sep, second)(input)
}

/// helper trait for the tuple combinator
///
/// this trait is implemented for tuples of parsers of up to 21 elements
pub trait Tuple<I,O,E> {
  /// parses the input and returns a tuple of results of each parser
  fn parse(&self, input: I) -> IResult<I,O,E>;
}

impl<Input, Output, Error: ParseError<Input>, F: Fn(Input) -> IResult<Input, Output, Error> > Tuple<Input, (Output,), Error> for (F,) {
   fn parse(&self, input: Input) -> IResult<Input,(Output,),Error> {
     self.0(input).map(|(i,o)| (i, (o,)))
   }
}

macro_rules! tuple_trait(
  ($name1:ident $ty1:ident, $name2: ident $ty2:ident, $($name:ident $ty:ident),*) => (
    tuple_trait!(__impl $name1 $ty1, $name2 $ty2; $($name $ty),*);
  );
  (__impl $($name:ident $ty: ident),+; $name1:ident $ty1:ident, $($name2:ident $ty2:ident),*) => (
    tuple_trait_impl!($($name $ty),+);
    tuple_trait!(__impl $($name $ty),+ , $name1 $ty1; $($name2 $ty2),*);
  );
  (__impl $($name:ident $ty: ident),+; $name1:ident $ty1:ident) => (
    tuple_trait_impl!($($name $ty),+);
    tuple_trait_impl!($($name $ty),+, $name1 $ty1);
  );
);

macro_rules! tuple_trait_impl(
  ($($name:ident $ty: ident),+) => (
    impl<
      Input: Clone, $($ty),+ , Error: ParseError<Input>,
      $($name: Fn(Input) -> IResult<Input, $ty, Error>),+
    > Tuple<Input, ( $($ty),+ ), Error> for ( $($name),+ ) {

      fn parse(&self, input: Input) -> IResult<Input, ( $($ty),+ ), Error> {
        tuple_trait_inner!(0, self, input, (), $($name)+)

      }
    }
  );
);

macro_rules! tuple_trait_inner(
  ($it:tt, $self:expr, $input:expr, (), $head:ident $($id:ident)+) => ({
    let (i, o) = $self.$it($input.clone())?;

    succ!($it, tuple_trait_inner!($self, i, ( o ), $($id)+))
  });
  ($it:tt, $self:expr, $input:expr, ($($parsed:tt)*), $head:ident $($id:ident)+) => ({
    let (i, o) = $self.$it($input.clone())?;

    succ!($it, tuple_trait_inner!($self, i, ($($parsed)* , o), $($id)+))
  });
  ($it:tt, $self:expr, $input:expr, ($($parsed:tt)*), $head:ident) => ({
    let (i, o) = $self.$it($input.clone())?;

    Ok((i, ($($parsed)* , o)))
  });
);

tuple_trait!(FnA A, FnB B, FnC C, FnD D, FnE E, FnF F, FnG G, FnH H, FnI I, FnJ J, FnK K, FnL L,
  FnM M, FnN N, FnO O, FnP P, FnQ Q, FnR R, FnS S, FnT T, FnU U);

/// applies a tuple of parsers one by one and returns their results as a tuple
///
/// ```rust
/// # use nom::{Err, error::ErrorKind};
/// use nom::sequence::tuple;
/// use nom::character::complete::{alpha1, digit1};
/// let parser = tuple((alpha1, digit1, alpha1));
///
/// assert_eq!(parser("abc123def"), Ok(("", ("abc", "123", "def"))));
/// assert_eq!(parser("123def"), Err(Err::Error(("123def", ErrorKind::Alpha))));
/// ```
pub fn tuple<I: Clone, O, E: ParseError<I>, List: Tuple<I,O,E>>(l: List)  -> impl Fn(I) -> IResult<I, O, E> {
  move |i: I| {
    l.parse(i)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn single_element_tuples() {
    use crate::character::complete::{alpha1, digit1};
    use crate::{Err, error::ErrorKind};

    let parser = tuple((alpha1,));
    assert_eq!(parser("abc123def"), Ok(("123def", ("abc",))));
    assert_eq!(parser("123def"), Err(Err::Error(("123def", ErrorKind::Alpha))));
  }
}
