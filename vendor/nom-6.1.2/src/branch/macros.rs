/// Try a list of parsers and return the result of the first successful one
///
/// ```rust,ignore
/// alt!(I -> IResult<I,O> | I -> IResult<I,O> | ... | I -> IResult<I,O> ) => I -> IResult<I, O>
/// ```
/// All the parsers must have the same return type.
///
/// If one of the parsers returns `Incomplete`, `alt!` will return `Incomplete`, to retry
/// once you get more input. Note that it is better for performance to know the
/// minimum size of data you need before you get into `alt!`.
///
/// The `alt!` combinator is used in the following way:
///
/// ```rust,ignore
/// alt!(parser_1 | parser_2 | ... | parser_n)
/// ```
///
/// # Basic example
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  // Create a parser that will match either "dragon" or "beast"
///  named!( dragon_or_beast, alt!( tag!( "dragon" ) | tag!( "beast" ) ) );
///
///  // Given the input "dragon slayer", the parser will match "dragon"
///  // and the rest will be " slayer"
///  let (rest, result) = dragon_or_beast(b"dragon slayer").unwrap();
///  assert_eq!(result, b"dragon");
///  assert_eq!(rest, b" slayer");
///
///  // Given the input "beast of Gevaudan", the parser will match "beast"
///  // and the rest will be " of Gevaudan"
///  let (rest, result) = dragon_or_beast(&b"beast of Gevaudan"[..]).unwrap();
///  assert_eq!(result, b"beast");
///  assert_eq!(rest, b" of Gevaudan");
///  # }
/// ```
///
/// # Manipulate results
///
/// There exists another syntax for `alt!` that gives you the ability to
/// manipulate the result from each parser:
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
/// #
/// // We create an enum to represent our creatures
/// #[derive(Debug,PartialEq,Eq)]
/// enum Creature {
///     Dragon,
///     Beast,
///     Unknown(usize)
/// }
///
/// // Let's make a helper function that returns true when not a space
/// // we are required to do this because the `take_while!` macro is limited
/// // to idents, so we can't negate `ìs_space` at the call site
/// fn is_not_space(c: u8) -> bool { ! nom::character::is_space(c) }
///
/// // Our parser will return the `Dragon` variant when matching "dragon",
/// // the `Beast` variant when matching "beast" and otherwise it will consume
/// // the input until a space is found and return an `Unknown` creature with
/// // the size of it's name.
/// named!(creature<Creature>, alt!(
///     tag!("dragon")            => { |_| Creature::Dragon } |
///     tag!("beast")             => { |_| Creature::Beast }  |
///     take_while!(is_not_space) => { |r: &[u8]| Creature::Unknown(r.len()) }
///     // the closure takes the result as argument if the parser is successful
/// ));
///
/// // Given the input "dragon slayer" the parser will return `Creature::Dragon`
/// // and the rest will be " slayer"
/// let (rest, result) = creature(b"dragon slayer").unwrap();
/// assert_eq!(result, Creature::Dragon);
/// assert_eq!(rest, b" slayer");
///
/// // Given the input "beast of Gevaudan" the parser will return `Creature::Beast`
/// // and the rest will be " of Gevaudan"
/// let (rest, result) = creature(b"beast of Gevaudan").unwrap();
/// assert_eq!(result, Creature::Beast);
/// assert_eq!(rest, b" of Gevaudan");
///
/// // Given the input "demon hunter" the parser will return `Creature::Unknown(5)`
/// // and the rest will be " hunter"
/// let (rest, result) = creature(b"demon hunter").unwrap();
/// assert_eq!(result, Creature::Unknown(5));
/// assert_eq!(rest, b" hunter");
/// # }
/// ```
///
/// # Behaviour of `alt!`
///
/// **BE CAREFUL** there is a case where the behaviour of `alt!` can be confusing:
///
/// When the alternatives have different lengths, like this case:
///
/// ```ignore
///  named!( test, alt!( tag!( "abcd" ) | tag!( "ef" ) | tag!( "ghi" ) | tag!( "kl" ) ) );
/// ```
///
/// With this parser, if you pass `"abcd"` as input, the first alternative parses it correctly,
/// but if you pass `"efg"`, the first alternative will return `Incomplete`, since it needs an input
/// of 4 bytes. This behaviour of `alt!` is expected: if you get a partial input that isn't matched
/// by the first alternative, but would match if the input was complete, you want `alt!` to indicate
/// that it cannot decide with limited information.
///
/// There are two ways to fix this behaviour. The first one consists in ordering the alternatives
/// by size, like this:
///
/// ```ignore
///  named!( test, alt!( tag!( "ef" ) | tag!( "kl") | tag!( "ghi" ) | tag!( "abcd" ) ) );
/// ```
///
/// With this solution, the largest alternative will be tested last.
///
/// The other solution uses the `complete!` combinator, which transforms an `Incomplete` in an
/// `Error`. If one of the alternatives returns `Incomplete` but is wrapped by `complete!`,
/// `alt!` will try the next alternative. This is useful when you know that
/// you will not get partial input:
///
/// ```ignore
///  named!( test,
///    alt!(
///      complete!( tag!( "abcd" ) ) |
///      complete!( tag!( "ef"   ) ) |
///      complete!( tag!( "ghi"  ) ) |
///      complete!( tag!( "kl"   ) )
///    )
///  );
/// ```
///
/// This behaviour of `alt!` can get especially confusing if multiple alternatives have different
/// sizes but a common prefix, like this:
///
/// ```ignore
///  named!( test, alt!( tag!( "abcd" ) | tag!( "ab" ) | tag!( "ef" ) ) );
/// ```
///
/// In that case, if you order by size, passing `"abcd"` as input will always be matched by the
/// smallest parser, so the solution using `complete!` is better suited.
///
/// You can also nest multiple `alt!`, like this:
///
/// ```ignore
///  named!( test,
///    alt!(
///      preceded!(
///        tag!("ab"),
///        alt!(
///          tag!( "cd" ) |
///          eof!()
///        )
///      )
///    | tag!( "ef" )
///    )
///  );
/// ```
///
///  `preceded!` will first parse `"ab"` then, if successful, try the alternatives "cd",
///  or empty input (End Of File). If none of them work, `preceded!` will fail and
///  "ef" will be tested.
///
#[macro_export(local_inner_macros)]
macro_rules! alt (
  (__impl $i:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)* ) => (
    nom_compile_error!("alt uses '|' as separator, not ',':

      alt!(
        tag!(\"abcd\") |
        tag!(\"efgh\") |
        tag!(\"ijkl\")
      )
    ");
  );
  (__impl $i:expr, $e:path, $($rest:tt)* ) => (
    alt!(__impl $i, call!($e) , $($rest)*);
  );
  (__impl $i:expr, $e:path | $($rest:tt)*) => (
    alt!(__impl $i, call!($e) | $($rest)*);
  );

  (__impl $i:expr, $subrule:ident!( $($args:tt)*) | $($rest:tt)*) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::Err;

      let i_ = $i.clone();
      let res = $subrule!(i_, $($args)*);
      match res {
        Ok(o) => Ok(o),
        Err(Err::Error(e))      => {
          let out = alt!(__impl $i, $($rest)*);

          // Compile-time hack to ensure that res's E type is not under-specified.
          // This all has no effect at runtime.
          #[allow(dead_code)]
          fn unify_types<T>(_: &T, _: &T) {}
          if let Err(Err::Error(ref e2)) = out {
            unify_types(&e, e2);
          }

          out
        },
        Err(e) => Err(e),
      }
    }
  );

  (__impl $i:expr, $subrule:ident!( $($args:tt)* ) => { $gen:expr } | $($rest:tt)*) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::Err;

      let i_ = $i.clone();
      match $subrule!(i_, $($args)* ) {
        Ok((i,o))         => Ok((i,$gen(o))),
        Err(Err::Error(e)) => {
          let out = alt!(__impl $i, $($rest)*);

          // Compile-time hack to ensure that res's E type is not under-specified.
          // This all has no effect at runtime.
          fn unify_types<T>(_: &T, _: &T) {}
          if let Err(Err::Error(ref e2)) = out {
            unify_types(&e, e2);
          }

          out
        },
        Err(e) => Err(e),
      }
    }
  );

  (__impl $i:expr, $e:path => { $gen:expr } | $($rest:tt)*) => (
    alt!(__impl $i, call!($e) => { $gen } | $($rest)*);
  );

  (__impl $i:expr, __end) => (
    {
      use $crate::{Err,error::ErrorKind};
      let e2 = ErrorKind::Alt;
      let err = Err::Error(error_position!($i, e2));

      Err(err)
    }
  );

  ($i:expr, $($rest:tt)*) => (
    {
      alt!(__impl $i, $($rest)* | __end)
    }
  );
);

/// `switch!(I -> IResult<I,P>, P => I -> IResult<I,O> | ... | P => I -> IResult<I,O> ) => I -> IResult<I, O>`
/// choose the next parser depending on the result of the first one, if successful,
/// and returns the result of the second parser
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # fn main() {
///  named!(sw,
///    switch!(take!(4),
///      b"abcd" => tag!("XYZ") |
///      b"efgh" => tag!("123")
///    )
///  );
///
///  let a = b"abcdXYZ123";
///  let b = b"abcdef";
///  let c = b"efgh123";
///  let d = b"blah";
///
///  assert_eq!(sw(&a[..]), Ok((&b"123"[..], &b"XYZ"[..])));
///  assert_eq!(sw(&b[..]), Err(Err::Error(error_node_position!(&b"abcdef"[..], ErrorKind::Switch,
///    error_position!(&b"ef"[..], ErrorKind::Tag)))));
///  assert_eq!(sw(&c[..]), Ok((&b""[..], &b"123"[..])));
///  assert_eq!(sw(&d[..]), Err(Err::Error(error_position!(&b"blah"[..], ErrorKind::Switch))));
///  # }
/// ```
///
/// You can specify a default case like with a normal match, using `_`
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(sw,
///    switch!(take!(4),
///      b"abcd" => tag!("XYZ") |
///      _       => value!(&b"default"[..])
///    )
///  );
///
///  let a = b"abcdXYZ123";
///  let b = b"blah";
///
///  assert_eq!(sw(&a[..]), Ok((&b"123"[..], &b"XYZ"[..])));
///  assert_eq!(sw(&b[..]), Ok((&b""[..], &b"default"[..])));
///  # }
/// ```
///
/// Due to limitations in Rust macros, it is not possible to have simple functions on the right hand
/// side of pattern, like this:
///
/// ```ignore
///  named!(xyz, tag!("XYZ"));
///  named!(num, tag!("123"));
///  named!(sw,
///    switch!(take!(4),
///      b"abcd" => xyz |
///      b"efgh" => 123
///    )
///  );
/// ```
///
/// If you want to pass your own functions instead, you can use the `call!` combinator as follows:
///
/// ```ignore
///  named!(xyz, tag!("XYZ"));
///  named!(num, tag!("123"));
///  named!(sw,
///    switch!(take!(4),
///      b"abcd" => call!(xyz) |
///      b"efgh" => call!(num)
///    )
///  );
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! switch (
  (__impl $i:expr, $submac:ident!( $($args:tt)* ), $( $($p:pat)|+ => $subrule:ident!( $($args2:tt)* ))|* ) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::lib::std::option::Option::*;
      use $crate::{Err,error::ErrorKind};

      let i_ = $i.clone();
      match map!(i_, $submac!($($args)*), Some) {
        Err(Err::Error(err))      => {
          fn unify_types<T>(_: &T, _: &T) {}
          let e1 = ErrorKind::Switch;
          let e2 = error_position!($i, e1.clone());
          unify_types(&err, &e2);

          Err(Err::Error(error_node_position!($i, e1, err)))
        },
        Err(e) => Err(e),
        Ok((i, o))    => {

          match o {
            $($(Some($p) )|+ => match $subrule!(i, $($args2)*) {
              Err(Err::Error(err)) => {
                fn unify_types<T>(_: &T, _: &T) {}
                let e1 = ErrorKind::Switch;
                let e2 = error_position!($i, e1.clone());
                unify_types(&err, &e2);

                Err(Err::Error(error_node_position!($i, e1, err)))
              },
              Ok(o) => Ok(o),
              Err(e) => Err(e),
            }),*,
            _    => Err(Err::convert(Err::Error(error_position!($i, ErrorKind::Switch))))
          }
        }
      }
    }
  );
  ($i:expr, $submac:ident!( $($args:tt)*), $($rest:tt)*) => (
    {
      switch!(__impl $i, $submac!($($args)*), $($rest)*)
    }
  );
  ($i:expr, $e:path, $($rest:tt)*) => (
    {
      switch!(__impl $i, call!($e), $($rest)*)
    }
  );
);

/// `permutation!(I -> IResult<I,A>, I -> IResult<I,B>, ... I -> IResult<I,X> ) => I -> IResult<I, (A,B,...X)>`
/// applies its sub parsers in a sequence, but independent from their order
/// this parser will only succeed if all of its sub parsers succeed.
///
/// The tuple of results is in the same order as the parsers are declared
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind,Needed};
/// # fn main() {
/// named!(perm<(&[u8], &[u8], &[u8])>,
///   permutation!(tag!("abcd"), tag!("efg"), tag!("hi"))
/// );
///
/// // whatever the order, if the parser succeeds, each
/// // tag should have matched correctly
/// let expected = (&b"abcd"[..], &b"efg"[..], &b"hi"[..]);
///
/// let a = &b"abcdefghijk"[..];
/// assert_eq!(perm(a), Ok((&b"jk"[..], expected)));
/// let b = &b"efgabcdhijkl"[..];
/// assert_eq!(perm(b), Ok((&b"jkl"[..], expected)));
/// let c = &b"hiefgabcdjklm"[..];
/// assert_eq!(perm(c), Ok((&b"jklm"[..], expected)));
///
/// let d = &b"efgxyzabcdefghi"[..];
/// assert_eq!(perm(d), Err(Err::Error(error_node_position!(&b"efgxyzabcdefghi"[..], ErrorKind::Permutation,
///   error_position!(&b"xyzabcdefghi"[..], ErrorKind::Permutation)))));
///
/// let e = &b"efgabc"[..];
/// assert_eq!(perm(e), Err(Err::Incomplete(Needed::new(1))));
/// # }
/// ```
///
/// If one of the child parsers is followed by a `?`, that parser is now
/// optional:
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind,Needed};
/// # fn main() {
/// named!(perm<&str, (Option<&str>, &str, &str)>,
///   permutation!(tag!("abcd")?, tag!("efg"), tag!("hi"))
/// );
///
/// // whatever the order, if the parser succeeds, each
/// // tag should have matched correctly
/// let expected = (Some("abcd"), "efg", "hi");
///
/// let a = "abcdefghijk";
/// assert_eq!(perm(a), Ok(("jk", expected)));
/// let b = "efgabcdhijkl";
/// assert_eq!(perm(b), Ok(("jkl", expected)));
/// let c = "hiefgabcdjklm";
/// assert_eq!(perm(c), Ok(("jklm", expected)));
///
/// // if `abcd` is missing:
/// let expected = (None, "efg", "hi");
///
/// let a = "efghijk";
/// assert_eq!(perm(a), Ok(("jk", expected)));
/// let b = "efghijkl";
/// assert_eq!(perm(b), Ok(("jkl", expected)));
/// let c = "hiefgjklm";
/// assert_eq!(perm(c), Ok(("jklm", expected)));
///
/// let e = "efgabc";
/// assert_eq!(perm(e), Err(Err::Incomplete(Needed::new(1))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! permutation (
  ($i:expr, $($rest:tt)*) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::lib::std::option::Option::*;
      use $crate::{Err,error::ErrorKind};

      let mut res    = permutation_init!((), $($rest)*);
      let mut input  = $i;
      let mut error  = None;
      let mut needed = None;

      loop {
        let mut all_done = true;
        permutation_iterator!(0, input, all_done, needed, res, $($rest)*);

        //if we reach that part, it means none of the parsers were able to read anything
        if !all_done {
          //FIXME: should wrap the error returned by the child parser
          error = Some(error_position!(input, ErrorKind::Permutation));
        }
        break;
      }

      if let Some(need) = needed {
        Err(Err::convert(need))
      } else {
        if let Some(unwrapped_res) = { permutation_unwrap!(0, (), res, $($rest)*) } {
          Ok((input, unwrapped_res))
        } else {
          if let Some(e) = error {
            Err(Err::Error(error_node_position!($i, ErrorKind::Permutation, e)))
          } else {
            Err(Err::Error(error_position!($i, ErrorKind::Permutation)))
          }
        }
      }
    }
  );
);

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! permutation_init (
  ((), $e:ident?, $($rest:tt)*) => (
    permutation_init!(($crate::lib::std::option::Option::None), $($rest)*)
  );
  ((), $e:ident, $($rest:tt)*) => (
    permutation_init!(($crate::lib::std::option::Option::None), $($rest)*)
  );

  ((), $submac:ident!( $($args:tt)* )?, $($rest:tt)*) => (
    permutation_init!(($crate::lib::std::option::Option::None), $($rest)*)
  );
  ((), $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    permutation_init!(($crate::lib::std::option::Option::None), $($rest)*)
  );

  (($($parsed:expr),*), $e:ident?, $($rest:tt)*) => (
    permutation_init!(($($parsed),* , $crate::lib::std::option::Option::None), $($rest)*);
  );
  (($($parsed:expr),*), $e:ident, $($rest:tt)*) => (
    permutation_init!(($($parsed),* , $crate::lib::std::option::Option::None), $($rest)*);
  );

  (($($parsed:expr),*), $submac:ident!( $($args:tt)* )?, $($rest:tt)*) => (
    permutation_init!(($($parsed),* , $crate::lib::std::option::Option::None), $($rest)*);
  );
  (($($parsed:expr),*), $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    permutation_init!(($($parsed),* , $crate::lib::std::option::Option::None), $($rest)*);
  );

  (($($parsed:expr),*), $e:ident) => (
    ($($parsed),* , $crate::lib::std::option::Option::None)
  );
  (($($parsed:expr),*), $e:ident?) => (
    ($($parsed),* , $crate::lib::std::option::Option::None)
  );

  (($($parsed:expr),*), $submac:ident!( $($args:tt)* )?) => (
    ($($parsed),* , $crate::lib::std::option::Option::None)
  );
  (($($parsed:expr),*), $submac:ident!( $($args:tt)* )) => (
    ($($parsed),* , $crate::lib::std::option::Option::None)
  );
  (($($parsed:expr),*),) => (
    ($($parsed),*)
  );
);

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! succ (
  (0, $submac:ident ! ($($rest:tt)*)) => ($submac!(1, $($rest)*));
  (1, $submac:ident ! ($($rest:tt)*)) => ($submac!(2, $($rest)*));
  (2, $submac:ident ! ($($rest:tt)*)) => ($submac!(3, $($rest)*));
  (3, $submac:ident ! ($($rest:tt)*)) => ($submac!(4, $($rest)*));
  (4, $submac:ident ! ($($rest:tt)*)) => ($submac!(5, $($rest)*));
  (5, $submac:ident ! ($($rest:tt)*)) => ($submac!(6, $($rest)*));
  (6, $submac:ident ! ($($rest:tt)*)) => ($submac!(7, $($rest)*));
  (7, $submac:ident ! ($($rest:tt)*)) => ($submac!(8, $($rest)*));
  (8, $submac:ident ! ($($rest:tt)*)) => ($submac!(9, $($rest)*));
  (9, $submac:ident ! ($($rest:tt)*)) => ($submac!(10, $($rest)*));
  (10, $submac:ident ! ($($rest:tt)*)) => ($submac!(11, $($rest)*));
  (11, $submac:ident ! ($($rest:tt)*)) => ($submac!(12, $($rest)*));
  (12, $submac:ident ! ($($rest:tt)*)) => ($submac!(13, $($rest)*));
  (13, $submac:ident ! ($($rest:tt)*)) => ($submac!(14, $($rest)*));
  (14, $submac:ident ! ($($rest:tt)*)) => ($submac!(15, $($rest)*));
  (15, $submac:ident ! ($($rest:tt)*)) => ($submac!(16, $($rest)*));
  (16, $submac:ident ! ($($rest:tt)*)) => ($submac!(17, $($rest)*));
  (17, $submac:ident ! ($($rest:tt)*)) => ($submac!(18, $($rest)*));
  (18, $submac:ident ! ($($rest:tt)*)) => ($submac!(19, $($rest)*));
  (19, $submac:ident ! ($($rest:tt)*)) => ($submac!(20, $($rest)*));
  (20, $submac:ident ! ($($rest:tt)*)) => ($submac!(21, $($rest)*));
);

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! permutation_unwrap (
  ($it:tt,  (), $res:ident, $e:ident?, $($rest:tt)*) => (
    succ!($it, permutation_unwrap!(($res.$it), $res, $($rest)*));
  );
  ($it:tt,  (), $res:ident, $e:ident, $($rest:tt)*) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_unwrap!((res.unwrap()), $res, $($rest)*))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt,  (), $res:ident, $submac:ident!( $($args:tt)* )?, $($rest:tt)*) => (
    succ!($it, permutation_unwrap!(($res.$it), $res, $($rest)*));
  );
  ($it:tt,  (), $res:ident, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_unwrap!((res.unwrap()), $res, $($rest)*))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident, $e:ident?, $($rest:tt)*) => (
    succ!($it, permutation_unwrap!(($($parsed),* , $res.$it), $res, $($rest)*));
  );
  ($it:tt, ($($parsed:expr),*), $res:ident, $e:ident, $($rest:tt)*) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_unwrap!(($($parsed),* , res.unwrap()), $res, $($rest)*))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident, $submac:ident!( $($args:tt)* )?, $($rest:tt)*) => (
    succ!($it, permutation_unwrap!(($($parsed),* , $res.$it), $res, $($rest)*));
  );
  ($it:tt, ($($parsed:expr),*), $res:ident, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => ({
    let res = $res.$it;
    if res.is_some() {
      succ!($it, permutation_unwrap!(($($parsed),* , res.unwrap()), $res, $($rest)*))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident?, $e:ident) => (
    $crate::lib::std::option::Option::Some(($($parsed),* , { $res.$it }))
  );
  ($it:tt, ($($parsed:expr),*), $res:ident, $e:ident) => ({
    let res = $res.$it;
    if res.is_some() {
      $crate::lib::std::option::Option::Some(($($parsed),* , res.unwrap() ))
    } else {
      $crate::lib::std::option::Option::None
    }
  });

  ($it:tt, ($($parsed:expr),*), $res:ident, $submac:ident!( $($args:tt)* )?) => (
    $crate::lib::std::option::Option::Some(($($parsed),* , { $res.$it }))
  );
  ($it:tt, ($($parsed:expr),*), $res:ident, $submac:ident!( $($args:tt)* )) => ({
    let res = $res.$it;
    if res.is_some() {
      $crate::lib::std::option::Option::Some(($($parsed),* , res.unwrap() ))
    } else {
      $crate::lib::std::option::Option::None
    }
  });
);

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! permutation_iterator (
  ($it:tt,$i:expr, $all_done:expr, $needed:expr, $res:expr, $e:ident?, $($rest:tt)*) => (
    permutation_iterator!($it, $i, $all_done, $needed, $res, call!($e), $($rest)*);
  );
  ($it:tt,$i:expr, $all_done:expr, $needed:expr, $res:expr, $e:ident, $($rest:tt)*) => (
    permutation_iterator!($it, $i, $all_done, $needed, $res, call!($e), $($rest)*);
  );

  ($it:tt, $i:expr, $all_done:expr, $needed:expr, $res:expr, $submac:ident!( $($args:tt)* )?, $($rest:tt)*) => {
    permutation_iterator!($it, $i, $all_done, $needed, $res, $submac!($($args)*) , $($rest)*);
  };
  ($it:tt, $i:expr, $all_done:expr, $needed:expr, $res:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => ({
    use $crate::lib::std::result::Result::*;
    use $crate::lib::std::option::Option::*;
    use $crate::Err;

    if $res.$it.is_none() {
      match $submac!($i, $($args)*) {
        Ok((i,o))     => {
          $i = i;
          $res.$it = Some(o);
          continue;
        },
        Err(Err::Error(_)) => {
          $all_done = false;
        },
        Err(e) => {
          $needed = Some(e);
          break;
        }
      };
    }
    succ!($it, permutation_iterator!($i, $all_done, $needed, $res, $($rest)*));
  });

  ($it:tt,$i:expr, $all_done:expr, $needed:expr, $res:expr, $e:ident?) => (
    permutation_iterator!($it, $i, $all_done, $needed, $res, call!($e));
  );
  ($it:tt,$i:expr, $all_done:expr, $needed:expr, $res:expr, $e:ident) => (
    permutation_iterator!($it, $i, $all_done, $needed, $res, call!($e));
  );

  ($it:tt, $i:expr, $all_done:expr, $needed:expr, $res:expr, $submac:ident!( $($args:tt)* )?) => {
    permutation_iterator!($it, $i, $all_done, $needed, $res, $submac!($($args)*));
  };
  ($it:tt, $i:expr, $all_done:expr, $needed:expr, $res:expr, $submac:ident!( $($args:tt)* )) => ({
    use $crate::lib::std::result::Result::*;
    use $crate::lib::std::option::Option::*;
    use $crate::Err;

    if $res.$it.is_none() {
      match $submac!($i, $($args)*) {
        Ok((i,o))     => {
          $i = i;
          $res.$it = Some(o);
          continue;
        },
        Err(Err::Error(_)) => {
          $all_done = false;
        },
        Err(e) => {
          $needed = Some(e);
          break;
        }
      };
    }
  });
);

#[cfg(test)]
mod tests {
  use crate::error::ErrorKind;
  use crate::internal::{Err, IResult, Needed};
  #[cfg(feature = "alloc")]
  use crate::{
    error::ParseError,
    lib::std::{
      fmt::Debug,
      string::{String, ToString},
    },
  };

  // reproduce the tag and take macros, because of module import order
  macro_rules! tag (
    ($i:expr, $inp: expr) => (
      {
        #[inline(always)]
        fn as_bytes<T: $crate::AsBytes>(b: &T) -> &[u8] {
          b.as_bytes()
        }

        let expected = $inp;
        let bytes    = as_bytes(&expected);

        tag_bytes!($i,bytes)
      }
    );
  );

  macro_rules! tag_bytes (
    ($i:expr, $bytes: expr) => (
      {
        use $crate::lib::std::cmp::min;

        let len = $i.len();
        let blen = $bytes.len();
        let m   = min(len, blen);
        let reduced = &$i[..m];
        let b       = &$bytes[..m];

        let res: IResult<_,_,_> = if reduced != b {
          let e: ErrorKind = ErrorKind::Tag;
          Err(Err::Error(error_position!($i, e)))
        } else if m < blen {
          Err(Err::Incomplete(Needed::new(blen)))
        } else {
          Ok((&$i[blen..], reduced))
        };
        res
      }
    );
  );

  macro_rules! take(
    ($i:expr, $count:expr) => (
      {
        let cnt = $count as usize;
        let res:IResult<&[u8],&[u8],_> = if $i.len() < cnt {
          Err(Err::Incomplete(Needed::new(cnt)))
        } else {
          Ok((&$i[cnt..],&$i[0..cnt]))
        };
        res
      }
    );
  );

  #[cfg(feature = "alloc")]
  #[derive(Debug, Clone, PartialEq)]
  pub struct ErrorStr(String);

  #[cfg(feature = "alloc")]
  impl From<u32> for ErrorStr {
    fn from(i: u32) -> Self {
      ErrorStr(format!("custom error code: {}", i))
    }
  }

  #[cfg(feature = "alloc")]
  impl<'a> From<&'a str> for ErrorStr {
    fn from(i: &'a str) -> Self {
      ErrorStr(format!("custom error message: {}", i))
    }
  }

  #[cfg(feature = "alloc")]
  impl<I: Debug> ParseError<I> for ErrorStr {
    fn from_error_kind(input: I, kind: ErrorKind) -> Self {
      ErrorStr(format!("custom error message: ({:?}, {:?})", input, kind))
    }

    fn append(input: I, kind: ErrorKind, other: Self) -> Self {
      ErrorStr(format!(
        "custom error message: ({:?}, {:?}) - {:?}",
        input, kind, other
      ))
    }
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn alt() {
    fn work(input: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      Ok((&b""[..], input))
    }

    #[allow(unused_variables)]
    fn dont_work(input: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      Err(Err::Error(ErrorStr("abcd".to_string())))
    }

    fn work2(input: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      Ok((input, &b""[..]))
    }

    fn alt1(i: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      alt!(i, dont_work | dont_work)
    }
    fn alt2(i: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      alt!(i, dont_work | work)
    }
    fn alt3(i: &[u8]) -> IResult<&[u8], &[u8], ErrorStr> {
      alt!(i, dont_work | dont_work | work2 | dont_work)
    }
    //named!(alt1, alt!(dont_work | dont_work));
    //named!(alt2, alt!(dont_work | work));
    //named!(alt3, alt!(dont_work | dont_work | work2 | dont_work));

    let a = &b"abcd"[..];
    assert_eq!(alt1(a), Err(Err::Error(error_position!(a, ErrorKind::Alt))));
    assert_eq!(alt2(a), Ok((&b""[..], a)));
    assert_eq!(alt3(a), Ok((a, &b""[..])));

    named!(alt4, alt!(tag!("abcd") | tag!("efgh")));
    let b = &b"efgh"[..];
    assert_eq!(alt4(a), Ok((&b""[..], a)));
    assert_eq!(alt4(b), Ok((&b""[..], b)));

    // test the alternative syntax
    named!(
      alt5<bool>,
      alt!(tag!("abcd") => { |_| false } | tag!("efgh") => { |_| true })
    );
    assert_eq!(alt5(a), Ok((&b""[..], false)));
    assert_eq!(alt5(b), Ok((&b""[..], true)));

    // compile-time test guarding against an underspecified E generic type (#474)
    named!(alt_eof1, alt!(eof!() | eof!()));
    named!(alt_eof2, alt!(eof!() => {|x| x} | eof!() => {|x| x}));
    let _ = (alt_eof1, alt_eof2);
  }

  #[test]
  fn alt_incomplete() {
    named!(alt1, alt!(tag!("a") | tag!("bc") | tag!("def")));

    let a = &b""[..];
    assert_eq!(alt1(a), Err(Err::Incomplete(Needed::new(1))));
    let a = &b"b"[..];
    assert_eq!(alt1(a), Err(Err::Incomplete(Needed::new(2))));
    let a = &b"bcd"[..];
    assert_eq!(alt1(a), Ok((&b"d"[..], &b"bc"[..])));
    let a = &b"cde"[..];
    assert_eq!(alt1(a), Err(Err::Error(error_position!(a, ErrorKind::Alt))));
    let a = &b"de"[..];
    assert_eq!(alt1(a), Err(Err::Incomplete(Needed::new(3))));
    let a = &b"defg"[..];
    assert_eq!(alt1(a), Ok((&b"g"[..], &b"def"[..])));
  }

  #[allow(unused_variables)]
  #[test]
  fn switch() {
    named!(
      sw,
      switch!(take!(4),
        b"abcd" | b"xxxx" => take!(2) |
        b"efgh" => take!(4)
      )
    );

    let a = &b"abcdefgh"[..];
    assert_eq!(sw(a), Ok((&b"gh"[..], &b"ef"[..])));

    let b = &b"efghijkl"[..];
    assert_eq!(sw(b), Ok((&b""[..], &b"ijkl"[..])));
    let c = &b"afghijkl"[..];
    assert_eq!(
      sw(c),
      Err(Err::Error(error_position!(
        &b"afghijkl"[..],
        ErrorKind::Switch
      )))
    );

    let a = &b"xxxxefgh"[..];
    assert_eq!(sw(a), Ok((&b"gh"[..], &b"ef"[..])));
  }

  #[test]
  fn permutation() {
    named!(
      perm<(&[u8], &[u8], &[u8])>,
      permutation!(tag!("abcd"), tag!("efg"), tag!("hi"))
    );

    let expected = (&b"abcd"[..], &b"efg"[..], &b"hi"[..]);

    let a = &b"abcdefghijk"[..];
    assert_eq!(perm(a), Ok((&b"jk"[..], expected)));
    let b = &b"efgabcdhijk"[..];
    assert_eq!(perm(b), Ok((&b"jk"[..], expected)));
    let c = &b"hiefgabcdjk"[..];
    assert_eq!(perm(c), Ok((&b"jk"[..], expected)));

    let d = &b"efgxyzabcdefghi"[..];
    assert_eq!(
      perm(d),
      Err(Err::Error(error_node_position!(
        &b"efgxyzabcdefghi"[..],
        ErrorKind::Permutation,
        error_position!(&b"xyzabcdefghi"[..], ErrorKind::Permutation)
      )))
    );

    let e = &b"efgabc"[..];
    assert_eq!(perm(e), Err(Err::Incomplete(Needed::new(4))));
  }

  /*
  named!(does_not_compile,
    alt!(tag!("abcd"), tag!("efgh"))
  );
  */
}
