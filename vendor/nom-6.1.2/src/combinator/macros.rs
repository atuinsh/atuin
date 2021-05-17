//! Macro combinators
//!
//! Macros are used to make combination easier,
//! since they often do not depend on the type
//! of the data they manipulate or return.
//!
//! There is a trick to make them easier to assemble,
//! combinators are defined like this:
//!
//! ```ignore
//! macro_rules! tag (
//!   ($i:expr, $inp: expr) => (
//!     {
//!       ...
//!     }
//!   );
//! );
//! ```
//!
//! But when used in other combinators, are used
//! like this:
//!
//! ```ignore
//! named!(my_function, tag!("abcd"));
//! ```
//!
//! Internally, other combinators will rewrite
//! that call to pass the input as first argument:
//!
//! ```ignore
//! macro_rules! named (
//!   ($name:ident, $submac:ident!( $($args:tt)* )) => (
//!     fn $name<'a>( i: &'a [u8] ) -> IResult<'a,&[u8], &[u8]> {
//!       $submac!(i, $($args)*)
//!     }
//!   );
//! );
//! ```
//!
//! If you want to call a combinator directly, you can
//! do it like this:
//!
//! ```ignore
//! let res = { tag!(input, "abcd"); }
//! ```
//!
//! Combinators must have a specific variant for
//! non-macro arguments. Example: Passing a function
//! to `take_while!` instead of another combinator.
//!
//! ```ignore
//! macro_rules! take_while(
//!   ($input:expr, $submac:ident!( $($args:tt)* )) => (
//!     {
//!       ...
//!     }
//!   );
//!
//!   // wrap the function in a macro to pass it to the main implementation
//!   ($input:expr, $f:expr) => (
//!     take_while!($input, call!($f));
//!   );
//! );
//! ```
#[allow(unused_variables)]

/// Makes a function from a parser combination
///
/// The type can be set up if the compiler needs
/// more information.
///
/// Function-like declaration:
/// ```
/// # use nom::{named, tag};
/// named!(my_function( &[u8] ) -> &[u8], tag!("abcd"));
/// ```
/// Alternative declaration. First type parameter is input, second is output:
/// ```
/// # use nom::{named, tag};
/// named!(my_function<&[u8], &[u8]>, tag!("abcd"));
/// ```
/// This one will have `&[u8]` as input type, `&[u8]` as output type:
/// ```
/// # use nom::{named, tag};
/// named!(my_function, tag!("abcd"));
/// ```
/// Will use `&[u8]` as output type:
/// ```
/// # use nom::{named, tag};
/// named!(my_function<&[u8]>, tag!("abcd"));
/// ```
/// Prefix them with 'pub' to make the functions public:
/// ```
/// # use nom::{named, tag};
/// named!(pub my_function, tag!("abcd"));
/// ```
/// Prefix them with 'pub(crate)' to make the functions public within the crate:
/// ```
/// # use nom::{named, tag};
/// named!(pub(crate) my_function, tag!("abcd"));
/// ```
#[macro_export(local_inner_macros)]
macro_rules! named (
    (#$($args:tt)*) => (
        named_attr!(#$($args)*);
    );
    ($vis:vis $name:ident( $i:ty ) -> $o:ty, $submac:ident!( $($args:tt)* )) => (
        $vis fn $name( i: $i ) -> $crate::IResult<$i, $o, $crate::error::Error<$i>> {
            $submac!(i, $($args)*)
        }
    );
    ($vis:vis $name:ident<$i:ty,$o:ty,$e:ty>, $submac:ident!( $($args:tt)* )) => (
        $vis fn $name( i: $i ) -> $crate::IResult<$i, $o, $e> {
            $submac!(i, $($args)*)
        }
    );
    ($vis:vis $name:ident<$i:ty,$o:ty>, $submac:ident!( $($args:tt)* )) => (
        $vis fn $name( i: $i ) -> $crate::IResult<$i, $o, $crate::error::Error<$i>> {
            $submac!(i, $($args)*)
        }
    );
    ($vis:vis $name:ident<$o:ty>, $submac:ident!( $($args:tt)* )) => (
        $vis fn $name( i: &[u8] ) -> $crate::IResult<&[u8], $o, $crate::error::Error<&[u8]>> {
            $submac!(i, $($args)*)
        }
    );
    ($vis:vis $name:ident, $submac:ident!( $($args:tt)* )) => (
        $vis fn $name( i: &[u8] ) -> $crate::IResult<&[u8], &[u8], $crate::error::Error<&[u8]>> {
            $submac!(i, $($args)*)
        }
    );
);

/// Makes a function from a parser combination with arguments.
///
/// ```ignore
/// //takes [`&[u8]`] as input
/// named_args!(tagged(open_tag: &[u8], close_tag: &[u8])<&str>,
///   delimited!(tag!(open_tag), map_res!(take!(4), str::from_utf8), tag!(close_tag))
/// );

/// //takes `&str` as input
/// named_args!(tagged(open_tag: &str, close_tag: &str)<&str, &str>,
///   delimited!(tag!(open_tag), take!(4), tag!(close_tag))
/// );
/// ```
///
/// Note: If using arguments that way gets hard to read, it is always
/// possible to write the equivalent parser definition manually, like
/// this:
///
/// ```ignore
/// fn tagged(input: &[u8], open_tag: &[u8], close_tag: &[u8]) -> IResult<&[u8], &str> {
///   // the first combinator in the tree gets the input as argument. It is then
///   // passed from one combinator to the next through macro rewriting
///   delimited!(input,
///     tag!(open_tag), take!(4), tag!(close_tag)
///   )
/// );
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! named_args {
    ($vis:vis $func_name:ident ( $( $arg:ident : $typ:ty ),* ) < $return_type:ty > , $submac:ident!( $($args:tt)* ) ) => {
        $vis fn $func_name(input: &[u8], $( $arg : $typ ),*) -> $crate::IResult<&[u8], $return_type> {
            $submac!(input, $($args)*)
        }
    };

    ($vis:vis $func_name:ident < 'a > ( $( $arg:ident : $typ:ty ),* ) < $return_type:ty > , $submac:ident!( $($args:tt)* ) ) => {
        $vis fn $func_name<'this_is_probably_unique_i_hope_please, 'a>(
          input: &'this_is_probably_unique_i_hope_please [u8], $( $arg : $typ ),*) ->
          $crate::IResult<&'this_is_probably_unique_i_hope_please [u8], $return_type>
        {
          $submac!(input, $($args)*)
        }
    };

    ($vis:vis $func_name:ident ( $( $arg:ident : $typ:ty ),* ) < $input_type:ty, $return_type:ty > , $submac:ident!( $($args:tt)* ) ) => {
        $vis fn $func_name(input: $input_type, $( $arg : $typ ),*) -> $crate::IResult<$input_type, $return_type> {
            $submac!(input, $($args)*)
        }
    };

    ($vis:vis $func_name:ident < 'a > ( $( $arg:ident : $typ:ty ),* ) < $input_type:ty, $return_type:ty > , $submac:ident!( $($args:tt)* ) ) => {
        $vis fn $func_name<'a>(
          input: $input_type, $( $arg : $typ ),*)
          -> $crate::IResult<$input_type, $return_type>
        {
            $submac!(input, $($args)*)
        }
    };
}

/// Makes a function from a parser combination, with attributes.
///
/// The usage of this macro is almost identical to `named!`, except that
/// you also pass attributes to be attached to the generated function.
/// This is ideal for adding documentation to your parser.
///
/// Create my_function as if you wrote it with the doc comment /// My Func:
/// ```
/// # use nom::{named_attr, tag};
/// named_attr!(#[doc = "My Func"], my_function( &[u8] ) -> &[u8], tag!("abcd"));
/// ```
/// Also works for pub functions, and multiple lines:
/// ```
/// # use nom::{named_attr, tag};
/// named_attr!(#[doc = "My Func\nRecognise abcd"], pub my_function, tag!("abcd"));
/// ```
/// Multiple attributes can be passed if required:
/// ```
/// # use nom::{named_attr, tag};
/// named_attr!(#[doc = "My Func"] #[inline(always)], pub my_function, tag!("abcd"));
/// ```
#[macro_export(local_inner_macros)]
macro_rules! named_attr (
    ($(#[$attr:meta])*, $vis:vis $name:ident( $i:ty ) -> $o:ty, $submac:ident!( $($args:tt)* )) => (
        $(#[$attr])*
        $vis fn $name( i: $i ) -> $crate::IResult<$i,$o, $crate::error::Error<$i>> {
            $submac!(i, $($args)*)
        }
    );
    ($(#[$attr:meta])*, $vis:vis $name:ident<$i:ty,$o:ty,$e:ty>, $submac:ident!( $($args:tt)* )) => (
        $(#[$attr])*
        $vis fn $name( i: $i ) -> $crate::IResult<$i, $o, $e> {
            $submac!(i, $($args)*)
        }
    );
    ($(#[$attr:meta])*, $vis:vis $name:ident<$i:ty,$o:ty>, $submac:ident!( $($args:tt)* )) => (
        $(#[$attr])*
        $vis fn $name( i: $i ) -> $crate::IResult<$i, $o, $crate::error::Error<$i>> {
            $submac!(i, $($args)*)
        }
    );
    ($(#[$attr:meta])*, $vis:vis $name:ident<$o:ty>, $submac:ident!( $($args:tt)* )) => (
        $(#[$attr])*
        $vis fn $name( i: &[u8] ) -> $crate::IResult<&[u8], $o, $crate::error::Error<&[u8]>> {
            $submac!(i, $($args)*)
        }
    );
    ($(#[$attr:meta])*, $vis:vis $name:ident, $submac:ident!( $($args:tt)* )) => (
        $(#[$attr])*
        $vis fn $name<'a>( i: &'a [u8] ) -> $crate::IResult<&[u8], &[u8], $crate::error::Error<&[u8]>> {
            $submac!(i, $($args)*)
        }
    );
);

/// Used to wrap common expressions and function as macros.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// # fn main() {
///   fn take_wrapper(input: &[u8], i: u8) -> IResult<&[u8], &[u8]> { take!(input, i * 10) }
///
///   // will make a parser taking 20 bytes
///   named!(parser, call!(take_wrapper, 2));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! call (
  ($i:expr, $fun:expr) => ( $fun( $i ) );
  ($i:expr, $fun:expr, $($args:expr),* ) => ( $fun( $i, $($args),* ) );
);

//FIXME: error rewrite
/// Prevents backtracking if the child parser fails.
///
/// This parser will do an early return instead of sending
/// its result to the parent parser.
///
/// If another `return_error!` combinator is present in the parent
/// chain, the error will be wrapped and another early
/// return will be made.
///
/// This makes it easy to build report on which parser failed,
/// where it failed in the input, and the chain of parsers
/// that led it there.
///
/// Additionally, the error chain contains number identifiers
/// that can be matched to provide useful error messages.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # fn main() {
///     named!(err_test<&[u8], &[u8]>, alt!(
///       tag!("abcd") |
///       preceded!(tag!("efgh"), return_error!(ErrorKind::Eof,
///           do_parse!(
///                  tag!("ijkl")                                        >>
///             res: return_error!(ErrorKind::Tag, tag!("mnop")) >>
///             (res)
///           )
///         )
///       )
///     ));
///     let a = &b"efghblah"[..];
///     let b = &b"efghijklblah"[..];
///     let c = &b"efghijklmnop"[..];
///
///     let blah = &b"blah"[..];
///
///     let res_a = err_test(a);
///     let res_b = err_test(b);
///     let res_c = err_test(c);
///     assert_eq!(res_a, Err(Err::Failure(error_node_position!(blah, ErrorKind::Eof, error_position!(blah, ErrorKind::Tag)))));
///     assert_eq!(res_b, Err(Err::Failure(error_node_position!(&b"ijklblah"[..], ErrorKind::Eof,
///       error_node_position!(blah, ErrorKind::Tag, error_position!(blah, ErrorKind::Tag))))
///     ));
/// # }
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! return_error (
  ($i:expr, $code:expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::Err;

      let i_ = $i.clone();
      let cl = || {
        $submac!(i_, $($args)*)
      };

      match cl() {
        Err(Err::Incomplete(x)) => Err(Err::Incomplete(x)),
        Ok((i, o))              => Ok((i, o)),
        Err(Err::Error(e)) | Err(Err::Failure(e)) => {
          return Err(Err::Failure($crate::error::append_error($i, $code, e)))
        }
      }
    }
  );
  ($i:expr, $code:expr, $f:expr) => (
    return_error!($i, $code, call!($f));
  );
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::Err;

      let i_ = $i.clone();
      let cl = || {
        $submac!(i_, $($args)*)
      };

      match cl() {
        Err(Err::Incomplete(x)) => Err(Err::Incomplete(x)),
        Ok((i, o))              => Ok((i, o)),
        Err(Err::Error(e)) | Err(Err::Failure(e)) => {
          return Err(Err::Failure(e))
        }
      }
    }
  );
  ($i:expr, $f:expr) => (
    return_error!($i, call!($f));
  );
);

//FIXME: error rewrite
/// Add an error if the child parser fails.
///
/// While `return_error!` does an early return and avoids backtracking,
/// `add_return_error!` backtracks normally. It just provides more context
/// for an error.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use std::collections;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # fn main() {
///     named!(err_test, add_return_error!(ErrorKind::Tag, tag!("abcd")));
///
///     let a = &b"efghblah"[..];
///     let res_a = err_test(a);
///     assert_eq!(res_a, Err(Err::Error(error_node_position!(a, ErrorKind::Tag, error_position!(a, ErrorKind::Tag)))));
/// # }
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! add_return_error (
  ($i:expr, $code:expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::{Err,error::ErrorKind};

      match $submac!($i, $($args)*) {
        Ok((i, o)) => Ok((i, o)),
        Err(Err::Error(e)) => {
          Err(Err::Error(error_node_position!($i, $code, e)))
        },
        Err(Err::Failure(e)) => {
          Err(Err::Failure(error_node_position!($i, $code, e)))
        },
        Err(e) => Err(e),
      }
    }
  );
  ($i:expr, $code:expr, $f:expr) => (
    add_return_error!($i, $code, call!($f));
  );
);

/// Replaces a `Incomplete` returned by the child parser
/// with an `Error`.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use std::collections;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # fn main() {
///     named!(take_5, complete!(take!(5)));
///
///     let a = &b"abcd"[..];
///     let res_a = take_5(a);
///     assert_eq!(res_a, Err(Err::Error(error_position!(a, ErrorKind::Complete))));
/// # }
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! complete (
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::completec($i, move |i| { $submac!(i, $($args)*) })
  );
  ($i:expr, $f:expr) => (
    complete!($i, call!($f));
  );
);

/// A bit like `std::try!`, this macro will return the remaining input and
/// parsed value if the child parser returned `Ok`, and will do an early
/// return for the `Err` side.
///
/// This can provide more flexibility than `do_parse!` if needed.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # use nom::IResult;
///
///  fn take_add(input:&[u8], size: u8) -> IResult<&[u8], &[u8]> {
///    let (i1, length)     = try_parse!(input, map_opt!(nom::number::streaming::be_u8, |sz| size.checked_add(sz)));
///    let (i2, data)   = try_parse!(i1, take!(length));
///    return Ok((i2, data));
///  }
/// # fn main() {
/// let arr1 = [1, 2, 3, 4, 5];
/// let r1 = take_add(&arr1[..], 1);
/// assert_eq!(r1, Ok((&[4,5][..], &[2,3][..])));
///
/// let arr2 = [0xFE, 2, 3, 4, 5];
/// // size is overflowing
/// let r1 = take_add(&arr2[..], 42);
/// assert_eq!(r1, Err(Err::Error(error_position!(&[254, 2,3,4,5][..], ErrorKind::MapOpt))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! try_parse (
  ($i:expr, $submac:ident!( $($args:tt)* )) => ({
    use $crate::lib::std::result::Result::*;

    match $submac!($i, $($args)*) {
      Ok((i,o)) => (i,o),
      Err(e)    => return Err(e),
    }
    });
  ($i:expr, $f:expr) => (
    try_parse!($i, call!($f))
  );
);

/// `map!(I -> IResult<I, O>, O -> P) => I -> IResult<I, P>`
///
/// Maps a function on the result of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// # fn main() {
///
/// named!(parse<&str, usize>, map!(digit1, |s| s.len()));
///
/// // the parser will count how many characters were returned by digit1
/// assert_eq!(parse("123456"), Ok(("", 6)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parse("abc"), Err(Err::Error(error_position!("abc", ErrorKind::Digit))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! map(
  // Internal parser, do not use directly
  (__impl $i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    $crate::combinator::mapc($i, move |i| {$submac!(i, $($args)*)}, $g)
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    map!(__impl $i, $submac!($($args)*), $g);
  );
  ($i:expr, $f:expr, $g:expr) => (
    map!(__impl $i, call!($f), $g);
  );
);

/// `map_res!(I -> IResult<I, O>, O -> Result<P>) => I -> IResult<I, P>`
/// maps a function returning a `Result` on the output of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind, IResult};
/// use nom::character::complete::digit1;
/// # fn main() {
///
/// named!(parse<&str, u8>, map_res!(digit1, |s: &str| s.parse::<u8>()));
///
/// // the parser will convert the result of digit1 to a number
/// assert_eq!(parse("123"), Ok(("", 123)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parse("abc"), Err(Err::Error(error_position!("abc", ErrorKind::Digit))));
///
/// // this will fail if the mapped function fails (a `u8` is too small to hold `123456`)
/// assert_eq!(parse("123456"), Err(Err::Error(error_position!("123456", ErrorKind::MapRes))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! map_res (
  // Internal parser, do not use directly
  (__impl $i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    $crate::combinator::map_resc($i, move |i| {$submac!(i, $($args)*)}, move |i| {$submac2!(i, $($args2)*)})
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    map_res!(__impl $i, $submac!($($args)*), call!($g));
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    map_res!(__impl $i, $submac!($($args)*), $submac2!($($args2)*));
  );
  ($i:expr, $f:expr, $g:expr) => (
    map_res!(__impl $i, call!($f), call!($g));
  );
  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    map_res!(__impl $i, call!($f), $submac!($($args)*));
  );
);

/// `map_opt!(I -> IResult<I, O>, O -> Option<P>) => I -> IResult<I, P>`
/// maps a function returning an `Option` on the output of a parser.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::{Error, ErrorKind}, IResult};
/// use nom::character::complete::digit1;
/// # fn main() {
///
/// named!(parser<&str, u8>, map_opt!(digit1, |s: &str| s.parse::<u8>().ok()));
///
/// // the parser will convert the result of digit1 to a number
/// assert_eq!(parser("123"), Ok(("", 123)));
///
/// // this will fail if digit1 fails
/// assert_eq!(parser("abc"), Err(Err::Error(Error::new("abc", ErrorKind::Digit))));
///
/// // this will fail if the mapped function fails (a `u8` is too small to hold `123456`)
/// assert_eq!(parser("123456"), Err(Err::Error(Error::new("123456", ErrorKind::MapOpt))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! map_opt (
  // Internal parser, do not use directly
  (__impl $i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    $crate::combinator::map_optc($i, move |i| {$submac!(i, $($args)*)}, move |i| {$submac2!(i, $($args2)*)})
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    map_opt!(__impl $i, $submac!($($args)*), call!($g));
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    map_opt!(__impl $i, $submac!($($args)*), $submac2!($($args2)*));
  );
  ($i:expr, $f:expr, $g:expr) => (
    map_opt!(__impl $i, call!($f), call!($g));
  );
  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    map_opt!(__impl $i, call!($f), $submac!($($args)*));
  );
);

/// `parse_to!(O) => I -> IResult<I, O>`
/// Uses the `parse` method from `std::str::FromStr` to convert the current
/// input to the specified type.
///
/// This will completely consume the input.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::{Error, ErrorKind}, IResult};
/// use nom::character::complete::digit1;
/// # fn main() {
///
/// named!(parser<&str, u8>, parse_to!(u8));
///
/// assert_eq!(parser("123"), Ok(("", 123)));
///
/// assert_eq!(parser("abc"), Err(Err::Error(Error::new("abc", ErrorKind::ParseTo))));
///
/// // this will fail if the mapped function fails (a `u8` is too small to hold `123456`)
/// assert_eq!(parser("123456"), Err(Err::Error(Error::new("123456", ErrorKind::ParseTo))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! parse_to (
  ($i:expr, $t:ty ) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::lib::std::option::Option;
      use $crate::lib::std::option::Option::*;
      use $crate::{Err,error::ErrorKind};

      use $crate::ParseTo;
      use $crate::Slice;
      use $crate::InputLength;

      let res: Option<$t> = ($i).parse_to();
      match res {
        Some(output) => Ok(($i.slice($i.input_len()..), output)),
        None         => Err(Err::Error($crate::error::make_error($i, ErrorKind::ParseTo)))
      }
    }
  );
);

/// `verify!(I -> IResult<I, O>, O -> bool) => I -> IResult<I, O>`
/// returns the result of the child parser if it satisfies a verification function.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(check<u32>, verify!(nom::number::streaming::be_u32, |val: &u32| *val < 3));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! verify (
  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    $crate::combinator::verifyc($i, |i| $submac!(i, $($args)*), $g)
  );
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    $crate::combinator::verifyc($i, |i| $submac!(i, $($args)*), |&o| $submac2!(o, $($args2)*))
  );
  ($i:expr, $f:expr, $g:expr) => (
    $crate::combinator::verify($f, $g)($i)
  );
  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::verify($f, |&o| $submac!(o, $($args)*))($i)
  );
);

/// `value!(T, R -> IResult<R, S> ) => R -> IResult<R, T>`
///
/// or `value!(T) => R -> IResult<R, T>`
///
/// If the child parser was successful, return the value.
/// If no child parser is provided, always return the value.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(x<u8>, value!(42, delimited!(tag!("<!--"), take!(5), tag!("-->"))));
///  named!(y<u8>, delimited!(tag!("<!--"), value!(42), tag!("-->")));
///  let r = x(&b"<!-- abc --> aaa"[..]);
///  assert_eq!(r, Ok((&b" aaa"[..], 42)));
///
///  let r2 = y(&b"<!----> aaa"[..]);
///  assert_eq!(r2, Ok((&b" aaa"[..], 42)));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! value (
  ($i:expr, $res:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::valuec($i, $res, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $res:expr, $f:expr) => (
    $crate::combinator::valuec($i, $res, $f)
  );
  ($i:expr, $res:expr) => (
    Ok(($i, $res))
  );
);

/// `opt!(I -> IResult<I,O>) => I -> IResult<I, Option<O>>`
/// make the underlying parser optional.
///
/// Returns an `Option` of the returned type. This parser returns `Some(result)` if the child parser
/// succeeds, `None` if it fails, and `Incomplete` if it did not have enough data to decide.
///
/// *Warning*: if you are using `opt` for some kind of optional ending token (like an end of line),
/// you should combine it with `complete` to make sure it works.
///
/// As an example, `opt!(tag!("\r\n"))` will return `Incomplete` if it receives an empty input,
/// because `tag` does not have enough input to decide.
/// On the contrary, `opt!(complete!(tag!("\r\n")))` would return `None` as produced value,
/// since `complete!` transforms an `Incomplete` in an `Error`.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!( o<&[u8], Option<&[u8]> >, opt!( tag!( "abcd" ) ) );
///
///  let a = b"abcdef";
///  let b = b"bcdefg";
///  assert_eq!(o(&a[..]), Ok((&b"ef"[..], Some(&b"abcd"[..]))));
///  assert_eq!(o(&b[..]), Ok((&b"bcdefg"[..], None)));
///  # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! opt(
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    {
      $crate::combinator::optc($i, |i| $submac!(i, $($args)*))
    }
  );
  ($i:expr, $f:expr) => (
    $crate::combinator::opt($f)($i)
  );
);

/// `opt_res!(I -> IResult<I,O>) => I -> IResult<I, Result<nom::Err,O>>`
/// make the underlying parser optional.
///
/// Returns a `Result`, with `Err` containing the parsing error.
///
/// ```ignore
/// # #[macro_use] extern crate nom;
/// # use nom::ErrorKind;
/// # fn main() {
///  named!( o<&[u8], Result<&[u8], nom::Err<&[u8]> > >, opt_res!( tag!( "abcd" ) ) );
///
///  let a = b"abcdef";
///  let b = b"bcdefg";
///  assert_eq!(o(&a[..]), Ok((&b"ef"[..], Ok(&b"abcd"[..])));
///  assert_eq!(o(&b[..]), Ok((&b"bcdefg"[..], Err(error_position!(&b[..], ErrorKind::Tag))));
///  # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! opt_res (
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::Err;

      let i_ = $i.clone();
      match $submac!(i_, $($args)*) {
        Ok((i,o))          => Ok((i,  Ok(o))),
        Err(Err::Error(e)) => Ok(($i, Err(Err::Error(e)))),
        // in case of failure, we return a real error
        Err(e)             => Err(e)
      }
    }
  );
  ($i:expr, $f:expr) => (
    opt_res!($i, call!($f));
  );
);

/// `cond!(bool, I -> IResult<I,O>) => I -> IResult<I, Option<O>>`
/// Conditional combinator
///
/// Wraps another parser and calls it if the
/// condition is met. This combinator returns
/// an Option of the return type of the child
/// parser.
///
/// This is especially useful if a parser depends
/// on the value returned by a preceding parser in
/// a `do_parse!`.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// # fn main() {
///  fn f_true(i: &[u8]) -> IResult<&[u8], Option<&[u8]>> {
///    cond!(i, true, tag!("abcd"))
///  }
///
///  fn f_false(i: &[u8]) -> IResult<&[u8], Option<&[u8]>> {
///    cond!(i, false, tag!("abcd"))
///  }
///
///  let a = b"abcdef";
///  assert_eq!(f_true(&a[..]), Ok((&b"ef"[..], Some(&b"abcd"[..]))));
///
///  assert_eq!(f_false(&a[..]), Ok((&b"abcdef"[..], None)));
///  # }
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! cond(
  ($i:expr, $cond:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::condc($i, $cond, |i|  $submac!(i, $($args)*) )
  );
  ($i:expr, $cond:expr, $f:expr) => (
    $crate::combinator::cond($cond, $f)($i)
  );
);

/// `peek!(I -> IResult<I,O>) => I -> IResult<I, O>`
/// returns a result without consuming the input.
///
/// The embedded parser may return `Err(Err::Incomplete)`.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(ptag, peek!( tag!( "abcd" ) ) );
///
///  let r = ptag(&b"abcdefgh"[..]);
///  assert_eq!(r, Ok((&b"abcdefgh"[..], &b"abcd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! peek(
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::peekc($i, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr) => (
    $crate::combinator::peek($f)($i)
  );
);

/// `not!(I -> IResult<I,O>) => I -> IResult<I, ()>`
/// returns a result only if the embedded parser returns `Error` or `Err(Err::Incomplete)`.
/// Does not consume the input.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # fn main() {
/// named!(not_e, do_parse!(
///     res: tag!("abc")      >>
///          not!(char!('e')) >>
///     (res)
/// ));
///
/// let r = not_e(&b"abcd"[..]);
/// assert_eq!(r, Ok((&b"d"[..], &b"abc"[..])));
///
/// let r2 = not_e(&b"abce"[..]);
/// assert_eq!(r2, Err(Err::Error(error_position!(&b"e"[..], ErrorKind::Not))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! not(
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::notc($i, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr) => (
    $crate::combinator::not($f)($i)
  );
);

/// `tap!(name: I -> IResult<I,O> => { block }) => I -> IResult<I, O>`
/// allows access to the parser's result without affecting it.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use std::str;
/// # fn main() {
///  named!(ptag, tap!(res: tag!( "abcd" ) => { println!("recognized {}", str::from_utf8(res).unwrap()) } ) );
///
///  let r = ptag(&b"abcdefgh"[..]);
///  assert_eq!(r, Ok((&b"efgh"[..], &b"abcd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! tap (
  ($i:expr, $name:ident : $submac:ident!( $($args:tt)* ) => $e:expr) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::{Err,Needed,IResult};

      match $submac!($i, $($args)*) {
        Ok((i,o)) => {
          let $name = o;
          $e;
          Ok((i, $name))
        },
        Err(e)    => Err(Err::convert(e)),
      }
    }
  );
  ($i:expr, $name: ident: $f:expr => $e:expr) => (
    tap!($i, $name: call!($f) => $e);
  );
);

/// `eof!()` returns its input if it is at the end of input data.
///
/// When we're at the end of the data, this combinator
/// will succeed.
///
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use std::str;
/// # use nom::{Err, error::{Error ,ErrorKind}};
/// # fn main() {
///  named!(parser, eof!());
///
///  assert_eq!(parser(&b"abc"[..]), Err(Err::Error(Error::new(&b"abc"[..], ErrorKind::Eof))));
///  assert_eq!(parser(&b""[..]), Ok((&b""[..], &b""[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! eof (
  ($i:expr,) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::{Err,error::ErrorKind};

      use $crate::InputLength;
      if ($i).input_len() == 0 {
        let clone = $i.clone();
        Ok(($i, clone))
      } else {
        Err(Err::Error(error_position!($i, ErrorKind::Eof)))
      }
    }
  );
);

/// `exact!()` will fail if the child parser does not consume the whole data.
///
/// TODO: example
#[macro_export(local_inner_macros)]
macro_rules! exact (
  ($i:expr, $submac:ident!( $($args:tt)* )) => ({
      terminated!($i, $submac!( $($args)*), eof!())
  });
  ($i:expr, $f:expr) => (
    exact!($i, call!($f));
  );
);

/// `recognize!(I -> IResult<I, O> ) => I -> IResult<I, I>`
/// if the child parser was successful, return the consumed input as produced value.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(x, recognize!(delimited!(tag!("<!--"), take!(5), tag!("-->"))));
///  let r = x(&b"<!-- abc --> aaa"[..]);
///  assert_eq!(r, Ok((&b" aaa"[..], &b"<!-- abc -->"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! recognize (
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::recognizec($i, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr) => (
    $crate::combinator::recognize($f)($i)
  );
);

/// `into!(I -> IResult<I, O1, E1>) => I -> IResult<I, O2, E2>`
/// automatically converts the child parser's result to another type
///
/// it will be able to convert the output value and the error value
/// as long as the `Into` implementations are available
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// # fn main() {
///  named!(parse_to_str<&str, &str>, take!(4));
///  named!(parse_to_vec<&str, Vec<u8>>, into!(parse_to_str));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! into (
  ($i:expr, $submac:ident!( $($args:tt)* )) => (
    $crate::combinator::intoc($i, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr) => (
    $crate::combinator::intoc($i, $f)
  );
);

#[cfg(test)]
mod tests {
  use crate::error::ErrorKind;
  use crate::error::ParseError;
  use crate::internal::{Err, IResult, Needed};
  #[cfg(feature = "alloc")]
  use crate::lib::std::boxed::Box;

  // reproduce the tag and take macros, because of module import order
  macro_rules! tag (
    ($i:expr, $tag: expr) => ({
      use $crate::lib::std::result::Result::*;
      use $crate::{Err,Needed,IResult,error::ErrorKind};
      use $crate::{Compare,CompareResult,InputLength,Slice};

      let res: IResult<_,_> = match ($i).compare($tag) {
        CompareResult::Ok => {
          let blen = $tag.input_len();
          Ok(($i.slice(blen..), $i.slice(..blen)))
        },
        CompareResult::Incomplete => {
          Err(Err::Incomplete(Needed::new($tag.input_len() - $i.input_len())))
        },
        CompareResult::Error => {
          let e:ErrorKind = ErrorKind::Tag;
          Err(Err::Error($crate::error::make_error($i, e)))
        }
      };
      res
      });
  );

  macro_rules! take(
    ($i:expr, $count:expr) => (
      {
        let cnt = $count as usize;
        let res:IResult<&[u8],&[u8]> = if $i.len() < cnt {
          Err($crate::Err::Incomplete($crate::Needed::new(cnt - $i.len())))
        } else {
          Ok((&$i[cnt..],&$i[0..cnt]))
        };
        res
      }
    );
  );

  mod pub_named_mod {
    named!(pub tst, tag!("abcd"));
  }

  #[test]
  fn pub_named_test() {
    let a = &b"abcd"[..];
    let res = pub_named_mod::tst(a);
    assert_eq!(res, Ok((&b""[..], a)));
  }

  mod pub_crate_named_mod {
    named!(pub(crate) tst, tag!("abcd"));
  }

  #[test]
  fn pub_crate_named_test() {
    let a = &b"abcd"[..];
    let res = pub_crate_named_mod::tst(a);
    assert_eq!(res, Ok((&b""[..], a)));
  }

  #[test]
  fn apply_test() {
    fn sum2(a: u8, b: u8) -> u8 {
      a + b
    }
    fn sum3(a: u8, b: u8, c: u8) -> u8 {
      a + b + c
    }
    let a = call!(1, sum2, 2);
    let b = call!(1, sum3, 2, 3);

    assert_eq!(a, 3);
    assert_eq!(b, 6);
  }

  #[test]
  fn opt() {
    named!(opt_abcd<&[u8],Option<&[u8]> >, opt!(tag!("abcd")));

    let a = &b"abcdef"[..];
    let b = &b"bcdefg"[..];
    let c = &b"ab"[..];
    assert_eq!(opt_abcd(a), Ok((&b"ef"[..], Some(&b"abcd"[..]))));
    assert_eq!(opt_abcd(b), Ok((&b"bcdefg"[..], None)));
    assert_eq!(opt_abcd(c), Err(Err::Incomplete(Needed::new(2))));
  }

  #[test]
  fn opt_res() {
    named!(opt_res_abcd<&[u8], Result<&[u8], Err<crate::error::Error<&[u8]>>> >, opt_res!(tag!("abcd")));

    let a = &b"abcdef"[..];
    let b = &b"bcdefg"[..];
    let c = &b"ab"[..];
    assert_eq!(opt_res_abcd(a), Ok((&b"ef"[..], Ok(&b"abcd"[..]))));
    assert_eq!(
      opt_res_abcd(b),
      Ok((
        &b"bcdefg"[..],
        Err(Err::Error(error_position!(b, ErrorKind::Tag)))
      ))
    );
    assert_eq!(opt_res_abcd(c), Err(Err::Incomplete(Needed::new(2))));
  }

  use crate::lib::std::convert::From;
  #[derive(Debug, PartialEq)]
  pub struct CustomError(&'static str);
  impl<I> From<(I, ErrorKind)> for CustomError {
    fn from(_: (I, ErrorKind)) -> Self {
      CustomError("test")
    }
  }
  impl<I> From<crate::error::Error<I>> for CustomError {
    fn from(_: crate::error::Error<I>) -> Self {
      CustomError("test")
    }
  }

  impl<I> ParseError<I> for CustomError {
    fn from_error_kind(_: I, _: ErrorKind) -> Self {
      CustomError("from_error_kind")
    }

    fn append(_: I, _: ErrorKind, _: CustomError) -> Self {
      CustomError("append")
    }
  }

  #[test]
  #[cfg(feature = "alloc")]
  fn cond() {
    fn f_true(i: &[u8]) -> IResult<&[u8], Option<&[u8]>, CustomError> {
      fix_error!(i, CustomError, cond!(true, tag!("abcd")))
    }

    fn f_false(i: &[u8]) -> IResult<&[u8], Option<&[u8]>, CustomError> {
      fix_error!(i, CustomError, cond!(false, tag!("abcd")))
    }

    assert_eq!(f_true(&b"abcdef"[..]), Ok((&b"ef"[..], Some(&b"abcd"[..]))));
    assert_eq!(f_true(&b"ab"[..]), Err(Err::Incomplete(Needed::new(2))));
    assert_eq!(f_true(&b"xxx"[..]), Err(Err::Error(CustomError("test"))));

    assert_eq!(f_false(&b"abcdef"[..]), Ok((&b"abcdef"[..], None)));
    assert_eq!(f_false(&b"ab"[..]), Ok((&b"ab"[..], None)));
    assert_eq!(f_false(&b"xxx"[..]), Ok((&b"xxx"[..], None)));
  }

  #[test]
  #[cfg(feature = "alloc")]
  fn cond_wrapping() {
    // Test that cond!() will wrap a given identifier in the call!() macro.
    named!(tag_abcd, tag!("abcd"));
    fn f_true(i: &[u8]) -> IResult<&[u8], Option<&[u8]>, CustomError> {
      fix_error!(i, CustomError, cond!(true, tag_abcd))
    }

    fn f_false(i: &[u8]) -> IResult<&[u8], Option<&[u8]>, CustomError> {
      fix_error!(i, CustomError, cond!(false, tag_abcd))
    }

    assert_eq!(f_true(&b"abcdef"[..]), Ok((&b"ef"[..], Some(&b"abcd"[..]))));
    assert_eq!(f_true(&b"ab"[..]), Err(Err::Incomplete(Needed::new(2))));
    assert_eq!(f_true(&b"xxx"[..]), Err(Err::Error(CustomError("test"))));

    assert_eq!(f_false(&b"abcdef"[..]), Ok((&b"abcdef"[..], None)));
    assert_eq!(f_false(&b"ab"[..]), Ok((&b"ab"[..], None)));
    assert_eq!(f_false(&b"xxx"[..]), Ok((&b"xxx"[..], None)));
  }

  #[test]
  fn peek() {
    named!(peek_tag<&[u8],&[u8]>, peek!(tag!("abcd")));

    assert_eq!(peek_tag(&b"abcdef"[..]), Ok((&b"abcdef"[..], &b"abcd"[..])));
    assert_eq!(peek_tag(&b"ab"[..]), Err(Err::Incomplete(Needed::new(2))));
    assert_eq!(
      peek_tag(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn not() {
    named!(not_aaa<()>, not!(tag!("aaa")));
    assert_eq!(
      not_aaa(&b"aaa"[..]),
      Err(Err::Error(error_position!(&b"aaa"[..], ErrorKind::Not)))
    );
    assert_eq!(not_aaa(&b"aa"[..]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(not_aaa(&b"abcd"[..]), Ok((&b"abcd"[..], ())));
  }

  #[test]
  fn verify() {
    named!(test, verify!(take!(5), |slice: &[u8]| slice[0] == b'a'));
    assert_eq!(test(&b"bcd"[..]), Err(Err::Incomplete(Needed::new(2))));
    assert_eq!(
      test(&b"bcdefg"[..]),
      Err(Err::Error(error_position!(
        &b"bcdefg"[..],
        ErrorKind::Verify
      )))
    );
    assert_eq!(test(&b"abcdefg"[..]), Ok((&b"fg"[..], &b"abcde"[..])));
  }

  #[test]
  fn parse_to() {
    let res: IResult<_, _, (&str, ErrorKind)> = parse_to!("ab", usize);

    assert_eq!(
      res,
      Err(Err::Error(error_position!("ab", ErrorKind::ParseTo)))
    );

    let res: IResult<_, _, (&str, ErrorKind)> = parse_to!("42", usize);

    assert_eq!(res, Ok(("", 42)));
    //assert_eq!(ErrorKind::convert(ErrorKind::ParseTo), ErrorKind::ParseTo::<u64>);
  }
}
