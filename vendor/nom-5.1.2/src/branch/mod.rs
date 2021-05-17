//! choice combinators

#[macro_use]
mod macros;

use crate::error::ErrorKind;
use crate::error::ParseError;
use crate::internal::{Err, IResult};

/// helper trait for the [alt()] combinator
///
/// this trait is implemented for tuples of up to 21 elements
pub trait Alt<I, O, E> {
  /// tests each parser in the tuple and returns the result of the first one that succeeds
  fn choice(&self, input: I) -> IResult<I, O, E>;
}

/// tests a list of parsers one by one until one succeeds
///
/// It takes as argument a tuple of parsers.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, Needed, IResult};
/// use nom::character::complete::{alpha1, digit1};
/// use nom::branch::alt;
/// # fn main() {
/// fn parser(input: &str) -> IResult<&str, &str> {
///   alt((alpha1, digit1))(input)
/// };
///
/// // the first parser, alpha1, recognizes the input
/// assert_eq!(parser("abc"), Ok(("", "abc")));
///
/// // the first parser returns an error, so alt tries the second one
/// assert_eq!(parser("123456"), Ok(("", "123456")));
///
/// // both parsers failed, and with the default error type, alt will return the last error
/// assert_eq!(parser(" "), Err(Err::Error(error_position!(" ", ErrorKind::Digit))));
/// # }
/// ```
///
/// with a custom error type, it is possible to have alt return the error of the parser
/// that went the farthest in the input data
pub fn alt<I: Clone, O, E: ParseError<I>, List: Alt<I, O, E>>(l: List) -> impl Fn(I) -> IResult<I, O, E> {
  move |i: I| l.choice(i)
}

/// helper trait for the [permutation()] combinator
///
/// this trait is implemented for tuples of up to 21 elements
pub trait Permutation<I, O, E> {
  /// tries to apply all parsers in the tuple in various orders until all of them succeed
  fn permutation(&self, input: I) -> IResult<I, O, E>;
}

/// applies a list of parsers in any order
///
/// permutation will succeed if all of the child parsers succeeded.
/// It takes as argument a tuple of parsers, and returns a
/// tuple of the parser results.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, Needed, IResult};
/// use nom::character::complete::{alpha1, digit1};
/// use nom::branch::permutation;
/// # fn main() {
/// fn parser(input: &str) -> IResult<&str, (&str, &str)> {
///   permutation((alpha1, digit1))(input)
/// };
///
/// // permutation recognizes alphabetic characters then digit
/// assert_eq!(parser("abc123"), Ok(("", ("abc", "123"))));
///
/// // but also in inverse order
/// assert_eq!(parser("123abc"), Ok(("", ("abc", "123"))));
///
/// // it will fail if one of the parsers failed
/// assert_eq!(parser("abc;"), Err(Err::Error(error_position!(";", ErrorKind::Permutation))));
/// # }
/// ```
pub fn permutation<I: Clone, O, E: ParseError<I>, List: Permutation<I, O, E>>(l: List) -> impl Fn(I) -> IResult<I, O, E> {
  move |i: I| l.permutation(i)
}

macro_rules! alt_trait(
  ($first:ident $second:ident $($id: ident)+) => (
    alt_trait!(__impl $first $second; $($id)+);
  );
  (__impl $($current:ident)*; $head:ident $($id: ident)+) => (
    alt_trait_impl!($($current)*);

    alt_trait!(__impl $($current)* $head; $($id)+);
  );
  (__impl $($current:ident)*; $head:ident) => (
    alt_trait_impl!($($current)*);
    alt_trait_impl!($($current)* $head);
  );
);

macro_rules! alt_trait_impl(
  ($($id:ident)+) => (
    impl<
      Input: Clone, Output, Error: ParseError<Input>,
      $($id: Fn(Input) -> IResult<Input, Output, Error>),+
    > Alt<Input, Output, Error> for ( $($id),+ ) {

      fn choice(&self, input: Input) -> IResult<Input, Output, Error> {
        let mut err: Option<Error> = None;
        alt_trait_inner!(0, self, input, err, $($id)+);

        Err(Err::Error(Error::append(input, ErrorKind::Alt, err.unwrap())))
      }
    }
  );
);

macro_rules! alt_trait_inner(
  ($it:tt, $self:expr, $input:expr, $err:expr, $head:ident $($id:ident)+) => (
    match $self.$it($input.clone()) {
      Err(Err::Error(e)) => {
        $err = Some(match $err.take() {
          None => e,
          Some(prev) => prev.or(e),
        });
        succ!($it, alt_trait_inner!($self, $input, $err, $($id)+))
      },
      res => return res,
    }
  );
  ($it:tt, $self:expr, $input:expr, $err:expr, $head:ident) => (
    match $self.$it($input.clone()) {
      Err(Err::Error(e)) => {
        $err = Some(match $err.take() {
          None => e,
          Some(prev) => prev.or(e),
        });
      },
      res => return res,
    }
  );
);

alt_trait!(A B C D E F G H I J K L M N O P Q R S T U);

macro_rules! permutation_trait(
  ($name1:ident $ty1:ident, $name2:ident $ty2:ident) => (
    permutation_trait_impl!($name1 $ty1, $name2 $ty2);
  );
  ($name1:ident $ty1:ident, $name2: ident $ty2:ident, $($name:ident $ty:ident),*) => (
    permutation_trait!(__impl $name1 $ty1, $name2 $ty2; $($name $ty),*);
  );
  (__impl $($name:ident $ty: ident),+; $name1:ident $ty1:ident, $($name2:ident $ty2:ident),*) => (
    permutation_trait_impl!($($name $ty),+);
    permutation_trait!(__impl $($name $ty),+ , $name1 $ty1; $($name2 $ty2),*);
  );
  (__impl $($name:ident $ty: ident),+; $name1:ident $ty1:ident) => (
    permutation_trait_impl!($($name $ty),+);
    permutation_trait_impl!($($name $ty),+, $name1 $ty1);
  );
);

macro_rules! permutation_trait_impl(
  ($($name:ident $ty: ident),+) => (
    impl<
      Input: Clone, $($ty),+ , Error: ParseError<Input>,
      $($name: Fn(Input) -> IResult<Input, $ty, Error>),+
    > Permutation<Input, ( $($ty),+ ), Error> for ( $($name),+ ) {

      fn permutation(&self, mut input: Input) -> IResult<Input, ( $($ty),+ ), Error> {
        let mut res = permutation_init!((), $($name),+);

        loop {
          let mut all_done = true;
          permutation_trait_inner!(0, self, input, res, all_done, $($name)+);

          //if we reach that part, it means none of the parsers were able to read anything
          if !all_done {
            //FIXME: should wrap the error returned by the child parser
            return Err(Err::Error(error_position!(input, ErrorKind::Permutation)));
          }
          break;
        }

        if let Some(unwrapped_res) = { permutation_trait_unwrap!(0, (), res, $($name),+) } {
          Ok((input, unwrapped_res))
        } else {
          Err(Err::Error(error_position!(input, ErrorKind::Permutation)))
        }
      }
    }
  );
);

macro_rules! permutation_trait_inner(
  ($it:tt, $self:expr, $input:ident, $res:expr, $all_done:expr, $head:ident $($id:ident)+) => ({
    if $res.$it.is_none() {
      match $self.$it($input.clone()) {
        Ok((i,o))     => {
          $input = i;
          $res.$it = Some(o);
          continue;
        },
        Err(Err::Error(_)) => {
          $all_done = false;
        },
        Err(e) => {
          return Err(e);
        }
      };
    }
    succ!($it, permutation_trait_inner!($self, $input, $res, $all_done, $($id)+));
  });
  ($it:tt, $self:expr, $input:ident, $res:expr, $all_done:expr, $head:ident) => ({
    if $res.$it.is_none() {
      match $self.$it($input.clone()) {
        Ok((i,o))     => {
          $input = i;
          $res.$it = Some(o);
          continue;
        },
        Err(Err::Error(_)) => {
          $all_done = false;
        },
        Err(e) => {
          return Err(e);
        }
      };
    }
  });
);

macro_rules! permutation_trait_unwrap (
  ($it:tt,  (), $res:ident, $e:ident, $($name:ident),+) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_trait_unwrap!((res.unwrap()), $res, $($name),+))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident, $e:ident, $($name:ident),+) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_trait_unwrap!(($($parsed),* , res.unwrap()), $res, $($name),+))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident, $name:ident) => ({
    let res = $res.$it;
    if res.is_some() {
      $crate::lib::std::option::Option::Some(($($parsed),* , res.unwrap() ))
    } else {
      $crate::lib::std::option::Option::None
    }
  });
);

permutation_trait!(FnA A, FnB B, FnC C, FnD D, FnE E, FnF F, FnG G, FnH H, FnI I, FnJ J, FnK K, FnL L, FnM M, FnN N, FnO O, FnP P, FnQ Q, FnR R, FnS S, FnT T, FnU U);
