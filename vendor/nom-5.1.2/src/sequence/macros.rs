/// `tuple!(I->IResult<I,A>, I->IResult<I,B>, ... I->IResult<I,X>) => I -> IResult<I, (A, B, ..., X)>`
/// chains parsers and assemble the sub results in a tuple.
///
/// The input type `I` must implement `nom::InputLength`.
///
/// This combinator will count how much data is consumed by every child parser
/// and take it into account if there is not enough data
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::error::ErrorKind;
/// # use nom::number::streaming::be_u16;
/// // the return type depends of the children parsers
/// named!(parser<&[u8], (u16, &[u8], &[u8]) >,
///   tuple!(
///     be_u16 ,
///     take!(3),
///     tag!("fg")
///   )
/// );
///
/// # fn main() {
/// assert_eq!(
///   parser(&b"abcdefgh"[..]),
///   Ok((
///     &b"h"[..],
///     (0x6162u16, &b"cde"[..], &b"fg"[..])
///   ))
/// );
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! tuple (
  ($i:expr, $($rest:tt)*) => (
    {
      tuple_parser!($i, (), $($rest)*)
    }
  );
);

/// Internal parser, do not use directly
#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! tuple_parser (
  ($i:expr, ($($parsed:tt),*), $e:path, $($rest:tt)*) => (
    tuple_parser!($i, ($($parsed),*), call!($e), $($rest)*);
  );
  ($i:expr, (), $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    {
      let i_ = $i.clone();

      ( $submac!(i_, $($args)*) ).and_then(|(i,o)| {
        let i_ = i.clone();
        tuple_parser!(i_, (o), $($rest)*)
      })
    }
  );
  ($i:expr, ($($parsed:tt)*), $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    {
      let i_ = $i.clone();

      ( $submac!(i_, $($args)*) ).and_then(|(i,o)| {
        let i_ = i.clone();
        tuple_parser!(i_, ($($parsed)* , o), $($rest)*)
      })
    }
  );
  ($i:expr, ($($parsed:tt),*), $e:path) => (
    tuple_parser!($i, ($($parsed),*), call!($e));
  );
  ($i:expr, (), $submac:ident!( $($args:tt)* )) => (
    {
      let i_ = $i.clone();
      ( $submac!(i_, $($args)*) ).map(|(i,o)| (i, (o)))
    }
  );
  ($i:expr, ($($parsed:expr),*), $submac:ident!( $($args:tt)* )) => (
    {
      let i_ = $i.clone();
      ( $submac!(i_, $($args)*) ).map(|(i,o)| (i, ($($parsed),* , o)))
    }
  );
  ($i:expr, ($($parsed:expr),*)) => (
    {
      $crate::lib::std::result::Result::Ok(($i, ($($parsed),*)))
    }
  );
);

/// `pair!(I -> IResult<I,O>, I -> IResult<I,P>) => I -> IResult<I, (O,P)>`
/// pair returns a tuple of the results of its two child parsers of both succeed
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # use nom::character::complete::{alpha1, digit1};
/// named!(parser<&str, (&str, &str)>, pair!(alpha1, digit1));
///
/// # fn main() {
/// assert_eq!(parser("abc123"), Ok(("", ("abc", "123"))));
/// assert_eq!(parser("123abc"), Err(Err::Error(("123abc", ErrorKind::Alpha))));
/// assert_eq!(parser("abc;123"), Err(Err::Error((";123", ErrorKind::Digit))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! pair(
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    pair!($i, |i| $submac!(i, $($args)*), |i| $submac2!(i, $($args2)*))
  );

  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    pair!($i, |i| $submac!(i, $($args)*), $g);
  );

  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    pair!($i, $f, |i| $submac!(i, $($args)*));
  );

  ($i:expr, $f:expr, $g:expr) => (
    $crate::sequence::pairc($i, $f, $g)
  );
);

/// `separated_pair!(I -> IResult<I,O>, I -> IResult<I, T>, I -> IResult<I,P>) => I -> IResult<I, (O,P)>`
/// separated_pair(X,sep,Y) returns a tuple of its first and third child parsers
/// if all 3 succeed
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # use nom::character::complete::{alpha1, digit1};
/// named!(parser<&str, (&str, &str)>, separated_pair!(alpha1, char!(','), digit1));
///
/// # fn main() {
/// assert_eq!(parser("abc,123"), Ok(("", ("abc", "123"))));
/// assert_eq!(parser("123,abc"), Err(Err::Error(("123,abc", ErrorKind::Alpha))));
/// assert_eq!(parser("abc;123"), Err(Err::Error((";123", ErrorKind::Char))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! separated_pair(
  ($i:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    separated_pair!($i, |i| $submac!(i, $($args)*), $($rest)*)
  );
  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    separated_pair!($i, $f, |i| $submac!(i, $($args)*), $($rest)*)
  );
  ($i:expr, $f:expr, $g:expr, $submac:ident!( $($args:tt)* )) => (
    separated_pair!($i, $f, $g, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr, $g:expr, $h:expr) => (
    $crate::sequence::separated_pairc($i, $f, $g, $h)
  );
);

/// `preceded!(I -> IResult<I,T>, I -> IResult<I,O>) => I -> IResult<I, O>`
/// preceded returns the result of its second parser if both succeed
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # use nom::character::complete::{alpha1};
/// named!(parser<&str, &str>, preceded!(char!('-'), alpha1));
///
/// # fn main() {
/// assert_eq!(parser("-abc"), Ok(("", "abc")));
/// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::Char))));
/// assert_eq!(parser("-123"), Err(Err::Error(("123", ErrorKind::Alpha))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! preceded(
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    preceded!($i, |i| $submac!(i, $($args)*), |i| $submac2!(i, $($args2)*))
  );

  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    preceded!($i, |i| $submac!(i, $($args)*), $g);
  );

  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    preceded!($i, $f, |i| $submac!(i, $($args)*));
  );

  ($i:expr, $f:expr, $g:expr) => (
    $crate::sequence::precededc($i, $f, $g)
  );
);

/// `terminated!(I -> IResult<I,O>, I -> IResult<I,T>) => I -> IResult<I, O>`
/// terminated returns the result of its first parser if both succeed
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::Err;
/// # use nom::error::ErrorKind;
/// # use nom::character::complete::{alpha1};
/// named!(parser<&str, &str>, terminated!(alpha1, char!(';')));
///
/// # fn main() {
/// assert_eq!(parser("abc;"), Ok(("", "abc")));
/// assert_eq!(parser("abc,"), Err(Err::Error((",", ErrorKind::Char))));
/// assert_eq!(parser("123;"), Err(Err::Error(("123;", ErrorKind::Alpha))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! terminated(
  ($i:expr, $submac:ident!( $($args:tt)* ), $submac2:ident!( $($args2:tt)* )) => (
    terminated!($i, |i| $submac!(i, $($args)*), |i| $submac2!(i, $($args2)*))
  );

  ($i:expr, $submac:ident!( $($args:tt)* ), $g:expr) => (
    terminated!($i, |i| $submac!(i, $($args)*), $g);
  );

  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* )) => (
    terminated!($i, $f, |i| $submac!(i, $($args)*));
  );

  ($i:expr, $f:expr, $g:expr) => (
    $crate::sequence::terminatedc($i, $f, $g)
  );
);

/// `delimited!(I -> IResult<I,T>, I -> IResult<I,O>, I -> IResult<I,U>) => I -> IResult<I, O>`
/// delimited(opening, X, closing) returns X
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::character::complete::{alpha1};
/// named!(parens,
///     delimited!(
///         tag!("("),
///         alpha1,
///         tag!(")")
///     )
/// );
///
/// # fn main() {
/// let input = &b"(test)"[..];
/// assert_eq!(parens(input), Ok((&b""[..], &b"test"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! delimited(
  ($i:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    delimited!($i, |i| $submac!(i, $($args)*), $($rest)*)
  );
  ($i:expr, $f:expr, $submac:ident!( $($args:tt)* ), $($rest:tt)*) => (
    delimited!($i, $f, |i| $submac!(i, $($args)*), $($rest)*)
  );
  ($i:expr, $f:expr, $g:expr, $submac:ident!( $($args:tt)* )) => (
    delimited!($i, $f, $g, |i| $submac!(i, $($args)*))
  );
  ($i:expr, $f:expr, $g:expr, $h:expr) => (
    $crate::sequence::delimitedc($i, $f, $g, $h)
  );
);

/// `do_parse!(I->IResult<I,A> >> I->IResult<I,B> >> ... I->IResult<I,X> , ( O ) ) => I -> IResult<I, O>`
/// do_parse applies sub parsers in a sequence.
/// it can store intermediary results and make them available
/// for later parsers
///
/// The input type `I` must implement `nom::InputLength`.
///
/// This combinator will count how much data is consumed by every child parser
/// and take it into account if there is not enough data
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,Needed};
/// use nom::number::streaming::be_u8;
///
/// // this parser implements a common pattern in binary formats,
/// // the TAG-LENGTH-VALUE, where you first recognize a specific
/// // byte slice, then the next bytes indicate the length of
/// // the data, then you take that slice and return it
/// //
/// // here, we match the tag 42, take the length in the next byte
/// // and store it in `length`, then use `take!` with `length`
/// // to obtain the subslice that we store in `bytes`, then return
/// // `bytes`
/// // you can use other macro combinators inside do_parse (like the `tag`
/// // one here), or a function (like `be_u8` here), but you cannot use a
/// // module path (like `nom::be_u8`) there, because of limitations in macros
/// named!(tag_length_value,
///   do_parse!(
///     tag!( &[ 42u8 ][..] ) >>
///     length: be_u8         >>
///     bytes:  take!(length) >>
///     (bytes)
///   )
/// );
///
/// # fn main() {
/// let a: Vec<u8>        = vec!(42, 2, 3, 4, 5);
/// let result_a: Vec<u8> = vec!(3, 4);
/// let rest_a: Vec<u8>   = vec!(5);
/// assert_eq!(tag_length_value(&a[..]), Ok((&rest_a[..], &result_a[..])));
///
/// // here, the length is 5, but there are only 3 bytes afterwards (3, 4 and 5),
/// // so the parser will tell you that you need 7 bytes: one for the tag,
/// // one for the length, then 5 bytes
/// let b: Vec<u8>     = vec!(42, 5, 3, 4, 5);
/// assert_eq!(tag_length_value(&b[..]), Err(Err::Incomplete(Needed::Size(5))));
/// # }
/// ```
///
/// the result is a tuple, so you can return multiple sub results, like
/// this:
/// `do_parse!(I->IResult<I,A> >> I->IResult<I,B> >> ... I->IResult<I,X> , ( O, P ) ) => I -> IResult<I, (O,P)>`
///
/// ```
/// # #[macro_use] extern crate nom;
/// use nom::number::streaming::be_u8;
/// named!(tag_length_value<(u8, &[u8])>,
///   do_parse!(
///     tag!( &[ 42u8 ][..] ) >>
///     length: be_u8         >>
///     bytes:  take!(length) >>
///     (length, bytes)
///   )
/// );
///
/// # fn main() {
/// # }
/// ```
///
#[macro_export(local_inner_macros)]
macro_rules! do_parse (
  (__impl $i:expr, ( $($rest:expr),* )) => (
    $crate::lib::std::result::Result::Ok(($i, ( $($rest),* )))
  );

  (__impl $i:expr, $field:ident : $submac:ident!( $($args:tt)* ) ) => (
    do_parse!(__impl $i, $submac!( $($args)* ))
  );

  (__impl $i:expr, $submac:ident!( $($args:tt)* ) ) => (
    nom_compile_error!("do_parse is missing the return value. A do_parse call must end
      with a return value between parenthesis, as follows:

      do_parse!(
        a: tag!(\"abcd\") >>
        b: tag!(\"efgh\") >>

        ( Value { a: a, b: b } )
    ");
  );

  (__impl $i:expr, $field:ident : $submac:ident!( $($args:tt)* ) ~ $($rest:tt)* ) => (
    nom_compile_error!("do_parse uses >> as separator, not ~");
  );
  (__impl $i:expr, $submac:ident!( $($args:tt)* ) ~ $($rest:tt)* ) => (
    nom_compile_error!("do_parse uses >> as separator, not ~");
  );
  (__impl $i:expr, $field:ident : $e:ident ~ $($rest:tt)*) => (
    do_parse!(__impl $i, $field: call!($e) ~ $($rest)*);
  );
  (__impl $i:expr, $e:ident ~ $($rest:tt)*) => (
    do_parse!(__impl $i, call!($e) ~ $($rest)*);
  );

  (__impl $i:expr, $e:ident >> $($rest:tt)*) => (
    do_parse!(__impl $i, call!($e) >> $($rest)*);
  );
  (__impl $i:expr, $submac:ident!( $($args:tt)* ) >> $($rest:tt)*) => (
    {
      use $crate::lib::std::result::Result::*;

      let i_ = $i.clone();
      match $submac!(i_, $($args)*) {
        Err(e) => Err(e),
        Ok((i,_))     => {
          let i_ = i.clone();
          do_parse!(__impl i_, $($rest)*)
        },
      }
    }
  );

  (__impl $i:expr, $field:ident : $e:ident >> $($rest:tt)*) => (
    do_parse!(__impl $i, $field: call!($e) >> $($rest)*);
  );

  (__impl $i:expr, $field:ident : $submac:ident!( $($args:tt)* ) >> $($rest:tt)*) => (
    {
      use $crate::lib::std::result::Result::*;

      let i_ = $i.clone();
      match  $submac!(i_, $($args)*) {
        Err(e) => Err(e),
        Ok((i,o))     => {
          let $field = o;
          let i_ = i.clone();
          do_parse!(__impl i_, $($rest)*)
        },
      }
    }
  );

  // ending the chain
  (__impl $i:expr, $e:ident >> ( $($rest:tt)* )) => (
    do_parse!(__impl $i, call!($e) >> ( $($rest)* ));
  );

  (__impl $i:expr, $submac:ident!( $($args:tt)* ) >> ( $($rest:tt)* )) => ({
    use $crate::lib::std::result::Result::*;

    match $submac!($i, $($args)*) {
      Err(e) => Err(e),
      Ok((i,_))     => {
        do_parse!(__finalize i, $($rest)*)
      },
    }
  });

  (__impl $i:expr, $field:ident : $e:ident >> ( $($rest:tt)* )) => (
    do_parse!(__impl $i, $field: call!($e) >> ( $($rest)* ) );
  );

  (__impl $i:expr, $field:ident : $submac:ident!( $($args:tt)* ) >> ( $($rest:tt)* )) => ({
    use $crate::lib::std::result::Result::*;

    match $submac!($i, $($args)*) {
      Err(e) => Err(e),
      Ok((i,o))     => {
        let $field = o;
        do_parse!(__finalize i, $($rest)*)
      },
    }
  });

  (__finalize $i:expr, ( $o: expr )) => ({
    use $crate::lib::std::result::Result::Ok;
    Ok(($i, $o))
  });

  (__finalize $i:expr, ( $($rest:tt)* )) => ({
    use $crate::lib::std::result::Result::Ok;
    Ok(($i, ( $($rest)* )))
  });

  ($i:expr, $($rest:tt)*) => (
    {
      do_parse!(__impl $i, $($rest)*)
    }
  );
  ($submac:ident!( $($args:tt)* ) >> $($rest:tt)* ) => (
    nom_compile_error!("if you are using do_parse outside of a named! macro, you must
        pass the input data as first argument, like this:

        let res = do_parse!(input,
          a: tag!(\"abcd\") >>
          b: tag!(\"efgh\") >>
          ( Value { a: a, b: b } )
        );");
  );
  ($e:ident! >> $($rest:tt)* ) => (
    do_parse!( call!($e) >> $($rest)*);
  );
);

#[doc(hidden)]
#[macro_export]
macro_rules! nom_compile_error (
  (( $($args:tt)* )) => ( compile_error!($($args)*) );
);

#[cfg(test)]
mod tests {
  use crate::internal::{Err, IResult, Needed};
  use crate::number::streaming::be_u16;
  use crate::error::ErrorKind;

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
          Err($crate::Err::Error(error_position!($i, $crate::error::ErrorKind::Tag)))
        } else if m < blen {
          Err($crate::Err::Incomplete(Needed::Size(blen)))
        } else {
          Ok((&$i[blen..], reduced))
        };
        res
      }
    );
  );

  macro_rules! take (
    ($i:expr, $count:expr) => (
      {
        let cnt = $count as usize;
        let res:IResult<&[u8],&[u8],_> = if $i.len() < cnt {
          Err($crate::Err::Incomplete(Needed::Size(cnt)))
        } else {
          Ok((&$i[cnt..],&$i[0..cnt]))
        };
        res
      }
    );
  );

  #[derive(PartialEq, Eq, Debug)]
  struct B {
    a: u8,
    b: u8,
  }

  #[derive(PartialEq, Eq, Debug)]
  struct C {
    a: u8,
    b: Option<u8>,
  }

  /*FIXME: convert code examples to new error management
  use util::{add_error_pattern, error_to_list, print_error};

  #[cfg(feature = "std")]
  #[cfg_attr(rustfmt, rustfmt_skip)]
  fn error_to_string<P: Clone + PartialEq>(e: &Context<P, u32>) -> &'static str {
    let v: Vec<(P, ErrorKind<u32>)> = error_to_list(e);
    // do it this way if you can use slice patterns
    //match &v[..] {
    //  [ErrorKind::Custom(42), ErrorKind::Tag]                         => "missing `ijkl` tag",
    //  [ErrorKind::Custom(42), ErrorKind::Custom(128), ErrorKind::Tag] => "missing `mnop` tag after `ijkl`",
    //  _            => "unrecognized error"
    //}

    let collected: Vec<ErrorKind<u32>> = v.iter().map(|&(_, ref e)| e.clone()).collect();
    if &collected[..] == [ErrorKind::Custom(42), ErrorKind::Tag] {
      "missing `ijkl` tag"
    } else if &collected[..] == [ErrorKind::Custom(42), ErrorKind::Custom(128), ErrorKind::Tag] {
      "missing `mnop` tag after `ijkl`"
    } else {
      "unrecognized error"
    }
  }

  // do it this way if you can use box patterns
  //use $crate::lib::std::str;
  //fn error_to_string(e:Err) -> String
  //  match e {
  //    NodePosition(ErrorKind::Custom(42), i1, box Position(ErrorKind::Tag, i2)) => {
  //      format!("missing `ijkl` tag, found '{}' instead", str::from_utf8(i2).unwrap())
  //    },
  //    NodePosition(ErrorKind::Custom(42), i1, box NodePosition(ErrorKind::Custom(128), i2,  box Position(ErrorKind::Tag, i3))) => {
  //      format!("missing `mnop` tag after `ijkl`, found '{}' instead", str::from_utf8(i3).unwrap())
  //    },
  //    _ => "unrecognized error".to_string()
  //  }
  //}
  */

  #[cfg_attr(rustfmt, rustfmt_skip)]
  #[allow(unused_variables)]
  #[test]
  fn add_err() {
    named!(err_test,
      preceded!(
        tag!("efgh"),
        add_return_error!(
          //ErrorKind::Custom(42u32),
          ErrorKind::Char,
          do_parse!(
                 tag!("ijkl")                                     >>
            //res: add_return_error!(ErrorKind::Custom(128u32), tag!("mnop")) >>
            res: add_return_error!(ErrorKind::Eof, tag!("mnop")) >>
            (res)
          )
        )
      )
    );
    let a = &b"efghblah"[..];
    let b = &b"efghijklblah"[..];
    let c = &b"efghijklmnop"[..];

    let blah = &b"blah"[..];

    let res_a = err_test(a);
    let res_b = err_test(b);
    let res_c = err_test(c);
    assert_eq!(res_a,
               Err(Err::Error(error_node_position!(blah,
                                                   //ErrorKind::Custom(42u32),
                                                   ErrorKind::Eof,
                                                   error_position!(blah, ErrorKind::Tag)))));
    //assert_eq!(res_b, Err(Err::Error(error_node_position!(&b"ijklblah"[..], ErrorKind::Custom(42u32),
    //  error_node_position!(blah, ErrorKind::Custom(128u32), error_position!(blah, ErrorKind::Tag))))));
    assert_eq!(res_b, Err(Err::Error(error_node_position!(&b"ijklblah"[..], ErrorKind::Eof,
      error_node_position!(blah, ErrorKind::Eof, error_position!(blah, ErrorKind::Tag))))));
    assert_eq!(res_c, Ok((&b""[..], &b"mnop"[..])));
  }

  #[cfg_attr(rustfmt, rustfmt_skip)]
  #[test]
  fn complete() {
    named!(err_test,
      do_parse!(
             tag!("ijkl")            >>
        res: complete!(tag!("mnop")) >>
        (res)
      )
    );
    let a = &b"ijklmn"[..];

    let res_a = err_test(a);
    assert_eq!(res_a,
               Err(Err::Error(error_position!(&b"mn"[..], ErrorKind::Complete))));
  }

  #[test]
  fn pair() {
    named!(tag_abc, tag!("abc"));
    named!(tag_def, tag!("def"));
    named!( pair_abc_def<&[u8],(&[u8], &[u8])>, pair!(tag_abc, tag_def) );

    assert_eq!(pair_abc_def(&b"abcdefghijkl"[..]), Ok((&b"ghijkl"[..], (&b"abc"[..], &b"def"[..]))));
    assert_eq!(pair_abc_def(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(pair_abc_def(&b"abcd"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(
      pair_abc_def(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      pair_abc_def(&b"xxxdef"[..]),
      Err(Err::Error(error_position!(&b"xxxdef"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      pair_abc_def(&b"abcxxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn separated_pair() {
    named!(tag_abc, tag!("abc"));
    named!(tag_def, tag!("def"));
    named!(tag_separator, tag!(","));
    named!( sep_pair_abc_def<&[u8],(&[u8], &[u8])>, separated_pair!(tag_abc, tag_separator, tag_def) );

    assert_eq!(
      sep_pair_abc_def(&b"abc,defghijkl"[..]),
      Ok((&b"ghijkl"[..], (&b"abc"[..], &b"def"[..])))
    );
    assert_eq!(sep_pair_abc_def(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(sep_pair_abc_def(&b"abc,d"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(
      sep_pair_abc_def(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      sep_pair_abc_def(&b"xxx,def"[..]),
      Err(Err::Error(error_position!(&b"xxx,def"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      sep_pair_abc_def(&b"abc,xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn preceded() {
    named!(tag_abcd, tag!("abcd"));
    named!(tag_efgh, tag!("efgh"));
    named!( preceded_abcd_efgh<&[u8], &[u8]>, preceded!(tag_abcd, tag_efgh) );

    assert_eq!(preceded_abcd_efgh(&b"abcdefghijkl"[..]), Ok((&b"ijkl"[..], &b"efgh"[..])));
    assert_eq!(preceded_abcd_efgh(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(preceded_abcd_efgh(&b"abcde"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(
      preceded_abcd_efgh(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      preceded_abcd_efgh(&b"xxxxdef"[..]),
      Err(Err::Error(error_position!(&b"xxxxdef"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      preceded_abcd_efgh(&b"abcdxxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn terminated() {
    named!(tag_abcd, tag!("abcd"));
    named!(tag_efgh, tag!("efgh"));
    named!( terminated_abcd_efgh<&[u8], &[u8]>, terminated!(tag_abcd, tag_efgh) );

    assert_eq!(terminated_abcd_efgh(&b"abcdefghijkl"[..]), Ok((&b"ijkl"[..], &b"abcd"[..])));
    assert_eq!(terminated_abcd_efgh(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(terminated_abcd_efgh(&b"abcde"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(
      terminated_abcd_efgh(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      terminated_abcd_efgh(&b"xxxxdef"[..]),
      Err(Err::Error(error_position!(&b"xxxxdef"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      terminated_abcd_efgh(&b"abcdxxxx"[..]),
      Err(Err::Error(error_position!(&b"xxxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn delimited() {
    named!(tag_abc, tag!("abc"));
    named!(tag_def, tag!("def"));
    named!(tag_ghi, tag!("ghi"));
    named!( delimited_abc_def_ghi<&[u8], &[u8]>, delimited!(tag_abc, tag_def, tag_ghi) );

    assert_eq!(delimited_abc_def_ghi(&b"abcdefghijkl"[..]), Ok((&b"jkl"[..], &b"def"[..])));
    assert_eq!(delimited_abc_def_ghi(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(delimited_abc_def_ghi(&b"abcde"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(delimited_abc_def_ghi(&b"abcdefgh"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(
      delimited_abc_def_ghi(&b"xxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      delimited_abc_def_ghi(&b"xxxdefghi"[..]),
      Err(Err::Error(error_position!(&b"xxxdefghi"[..], ErrorKind::Tag),))
    );
    assert_eq!(
      delimited_abc_def_ghi(&b"abcxxxghi"[..]),
      Err(Err::Error(error_position!(&b"xxxghi"[..], ErrorKind::Tag)))
    );
    assert_eq!(
      delimited_abc_def_ghi(&b"abcdefxxx"[..]),
      Err(Err::Error(error_position!(&b"xxx"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn tuple_test() {
    named!(tuple_3<&[u8], (u16, &[u8], &[u8]) >,
    tuple!( be_u16 , take!(3), tag!("fg") ) );

    assert_eq!(tuple_3(&b"abcdefgh"[..]), Ok((&b"h"[..], (0x6162u16, &b"cde"[..], &b"fg"[..]))));
    assert_eq!(tuple_3(&b"abcd"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(tuple_3(&b"abcde"[..]), Err(Err::Incomplete(Needed::Size(2))));
    assert_eq!(
      tuple_3(&b"abcdejk"[..]),
      Err(Err::Error(error_position!(&b"jk"[..], ErrorKind::Tag)))
    );
  }

  #[test]
  fn do_parse() {
    fn ret_int1(i: &[u8]) -> IResult<&[u8], u8> {
      Ok((i, 1))
    };
    fn ret_int2(i: &[u8]) -> IResult<&[u8], u8> {
      Ok((i, 2))
    };

    //trace_macros!(true);
    named!(do_parser<&[u8], (u8, u8)>,
      do_parse!(
        tag!("abcd")       >>
        opt!(tag!("abcd")) >>
        aa: ret_int1       >>
        tag!("efgh")       >>
        bb: ret_int2       >>
        tag!("efgh")       >>
        (aa, bb)
      )
    );
    //named!(do_parser<&[u8], (u8, u8)>,
    //  do_parse!(
    //    tag!("abcd") >> aa: ret_int1 >> tag!("efgh") >> bb: ret_int2 >> tag!("efgh") >> (aa, bb)
    //  )
    //);

    //trace_macros!(false);

    assert_eq!(do_parser(&b"abcdabcdefghefghX"[..]), Ok((&b"X"[..], (1, 2))));
    assert_eq!(do_parser(&b"abcdefghefghX"[..]), Ok((&b"X"[..], (1, 2))));
    assert_eq!(do_parser(&b"abcdab"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(do_parser(&b"abcdefghef"[..]), Err(Err::Incomplete(Needed::Size(4))));
  }

  #[cfg_attr(rustfmt, rustfmt_skip)]
  #[test]
  fn do_parse_dependency() {
    use crate::number::streaming::be_u8;

    named!(length_value,
      do_parse!(
        length: be_u8         >>
        bytes:  take!(length) >>
        (bytes)
      )
    );

    let a = [2u8, 3, 4, 5];
    let res_a = [3u8, 4];
    assert_eq!(length_value(&a[..]), Ok((&a[3..], &res_a[..])));
    let b = [5u8, 3, 4, 5];
    assert_eq!(length_value(&b[..]), Err(Err::Incomplete(Needed::Size(5))));
  }

  /*
  named!(does_not_compile,
    do_parse!(
      length: be_u8         >>
      bytes:  take!(length)
    )
  );
  named!(does_not_compile_either,
    do_parse!(
      length: be_u8         ~
      bytes:  take!(length) ~
      ( () )
    )
  );
  fn still_does_not_compile() {
    let data = b"abcd";

    let res = do_parse!(
      tag!("abcd") >>
      tag!("efgh") >>
      ( () )
    );
  }
  */
}
