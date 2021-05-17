//! Byte level parsers and combinators
//!
#[allow(unused_variables)]

/// `tag!(&[T]: nom::AsBytes) => &[T] -> IResult<&[T], &[T]>`
/// declares a byte array as a suite to recognize
///
/// consumes the recognized characters
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(x, tag!("abcd"));
///  let r = x(&b"abcdefgh"[..]);
///  assert_eq!(r, Ok((&b"efgh"[..], &b"abcd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! tag (
  ($i:expr, $tag: expr) => ({
    $crate::bytes::streaming::tag($tag)($i)
  });
);

/// `tag_no_case!(&[T]) => &[T] -> IResult<&[T], &[T]>`
/// declares a case insensitive ascii string as a suite to recognize
///
/// consumes the recognized characters
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(test, tag_no_case!("ABcd"));
///
///  let r = test(&b"aBCdefgh"[..]);
///  assert_eq!(r, Ok((&b"efgh"[..], &b"aBCd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! tag_no_case (
  ($i:expr, $tag: expr) => ({
    $crate::bytes::streaming::tag_no_case($tag)($i)
  });
);

/// `is_not!(&[T:AsBytes]) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest list of bytes that do not appear in the provided array
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!( not_space, is_not!( " \t\r\n" ) );
///
///  let r = not_space(&b"abcdefgh\nijkl"[..]);
///  assert_eq!(r, Ok((&b"\nijkl"[..], &b"abcdefgh"[..])));
///  # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! is_not (
  ($input:expr, $arr:expr) => ({
    $crate::bytes::streaming::is_not($arr)($input)
  });
);

/// `is_a!(&[T]) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest list of bytes that appear in the provided array
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(abcd, is_a!( "abcd" ));
///
///  let r1 = abcd(&b"aaaaefgh"[..]);
///  assert_eq!(r1, Ok((&b"efgh"[..], &b"aaaa"[..])));
///
///  let r2 = abcd(&b"dcbaefgh"[..]);
///  assert_eq!(r2, Ok((&b"efgh"[..], &b"dcba"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! is_a (
  ($input:expr, $arr:expr) => ({
    $crate::bytes::streaming::is_a($arr)($input)
  });
);

/// `escaped!(T -> IResult<T, T>, U, T -> IResult<T, T>) => T -> IResult<T, T> where T: InputIter,
/// U: AsChar`
/// matches a byte string with escaped characters.
///
/// The first argument matches the normal characters (it must not accept the control character),
/// the second argument is the control character (like `\` in most languages),
/// the third argument matches the escaped characters
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::character::complete::digit1;
/// # fn main() {
///  named!(esc, escaped!(call!(digit1), '\\', one_of!("\"n\\")));
///  assert_eq!(esc(&b"123;"[..]), Ok((&b";"[..], &b"123"[..])));
///  assert_eq!(esc(&b"12\\\"34;"[..]), Ok((&b";"[..], &b"12\\\"34"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! escaped (
  ($i:expr, $submac1:ident!( $($args:tt)* ), $control_char: expr, $submac2:ident!( $($args2:tt)*) ) => (
    {
     escaped!($i, |i| $submac1!(i, $($args)*), $control_char,  |i| $submac2!(i, $($args2)*))
    }
  );
  ($i:expr, $normal:expr, $control_char: expr, $submac2:ident!( $($args2:tt)*) ) => (
    {
     escaped!($i, $normal, $control_char,  |i| $submac2!(i, $($args2)*))
    }
  );
  ($i:expr, $submac1:ident!( $($args:tt)* ), $control_char: expr, $escapable:expr ) => (
    {
     escaped!($i, |i| $submac1!(i, $($args)*), $control_char,  $escapable)
    }
  );
  ($i:expr, $normal:expr, $control_char: expr, $escapable:expr) => (
    {
      $crate::bytes::complete::escapedc($i, $normal, $control_char, $escapable)
    }
  );
);

/// `escaped_transform!(&[T] -> IResult<&[T], &[T]>, T, &[T] -> IResult<&[T], &[T]>) => &[T] -> IResult<&[T], Vec<T>>`
/// matches a byte string with escaped characters.
///
/// The first argument matches the normal characters (it must not match the control character),
/// the second argument is the control character (like `\` in most languages),
/// the third argument matches the escaped characters and transforms them.
///
/// As an example, the chain `abc\tdef` could be `abc    def` (it also consumes the control character)
///
/// # Example
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::character::complete::alpha1;
/// # use nom::lib::std::str::from_utf8;
/// # fn main() {
/// fn to_s(i:Vec<u8>) -> String {
///   String::from_utf8_lossy(&i).into_owned()
/// }
///
///  named!(transform < String >,
///    map!(
///      escaped_transform!(call!(alpha1), '\\',
///        alt!(
///            tag!("\\")       => { |_| &b"\\"[..] }
///          | tag!("\"")       => { |_| &b"\""[..] }
///          | tag!("n")        => { |_| &b"\n"[..] }
///        )
///      ), to_s
///    )
///  );
///  assert_eq!(transform(&b"ab\\\"cd"[..]), Ok((&b""[..], String::from("ab\"cd"))));
/// # }
/// ```
#[cfg(feature = "alloc")]
#[macro_export(local_inner_macros)]
macro_rules! escaped_transform (
  ($i:expr, $submac1:ident!( $($args:tt)* ), $control_char: expr, $submac2:ident!( $($args2:tt)*) ) => (
    {
     escaped_transform!($i, |i| $submac1!(i, $($args)*), $control_char,  |i| $submac2!(i, $($args2)*))
    }
  );
  ($i:expr, $normal:expr, $control_char: expr, $submac2:ident!( $($args2:tt)*) ) => (
    {
     escaped_transform!($i, $normal, $control_char,  |i| $submac2!(i, $($args2)*))
    }
  );
  ($i:expr, $submac1:ident!( $($args:tt)* ), $control_char: expr, $transform:expr ) => (
    {
     escaped_transform!($i, |i| $submac1!(i, $($args)*), $control_char,  $transform)
    }
  );
  ($i:expr, $normal:expr, $control_char: expr, $transform:expr) => (
    {
      $crate::bytes::complete::escaped_transformc($i, $normal, $control_char, $transform)
    }
  );
);

/// `take_while!(T -> bool) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest list of bytes until the provided function fails.
///
/// The argument is either a function `T -> bool` or a macro returning a `bool`.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::character::is_alphanumeric;
/// # fn main() {
///  named!( alpha, take_while!( is_alphanumeric ) );
///
///  let r = alpha(&b"abcd\nefgh"[..]);
///  assert_eq!(r, Ok((&b"\nefgh"[..], &b"abcd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_while (
  ($input:expr, $submac:ident!( $($args:tt)* )) => ({
    let res: $crate::IResult<_, _, _> = take_while!($input, (|c| $submac!(c, $($args)*)));
    res
  });
  ($input:expr, $f:expr) => (
    $crate::bytes::streaming::take_while($f)($input)
  );
);

/// `take_while1!(T -> bool) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest (non empty) list of bytes until the provided function fails.
///
/// The argument is either a function `&[T] -> bool` or a macro returning a `bool`
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err,error::ErrorKind};
/// # use nom::character::is_alphanumeric;
/// # fn main() {
///  named!( alpha, take_while1!( is_alphanumeric ) );
///
///  let r = alpha(&b"abcd\nefgh"[..]);
///  assert_eq!(r, Ok((&b"\nefgh"[..], &b"abcd"[..])));
///  let r = alpha(&b"\nefgh"[..]);
///  assert_eq!(r, Err(Err::Error(error_position!(&b"\nefgh"[..], ErrorKind::TakeWhile1))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_while1 (
  ($input:expr, $submac:ident!( $($args:tt)* )) => ({
    let res: $crate::IResult<_, _, _> = take_while1!($input, (|c| $submac!(c, $($args)*)));
    res
  });
  ($input:expr, $f:expr) => (
    $crate::bytes::streaming::take_while1($f)($input)
  );
);

/// `take_while_m_n!(m: usize, n: usize, T -> bool) => &[T] -> IResult<&[T], &[T]>`
/// returns a list of bytes or characters for which the provided function returns true.
/// the returned list's size will be at least m, and at most n
///
/// The argument is either a function `T -> bool` or a macro returning a `bool`.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::character::is_alphanumeric;
/// # fn main() {
///  named!( alpha, take_while_m_n!(3, 6, is_alphanumeric ) );
///
///  let r = alpha(&b"abcd\nefgh"[..]);
///  assert_eq!(r, Ok((&b"\nefgh"[..], &b"abcd"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_while_m_n (
  ($input:expr, $m:expr, $n: expr, $submac:ident!( $($args:tt)* )) => ({
    let res: $crate::IResult<_, _, _> = take_while_m_n!($input, $m, $n, (|c| $submac!(c, $($args)*)));
    res
  });
  ($input:expr, $m:expr, $n:expr, $f:expr) => (
    $crate::bytes::streaming::take_while_m_n($m, $n, $f)($input)
  );
);

/// `take_till!(T -> bool) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest list of bytes until the provided function succeeds
///
/// The argument is either a function `&[T] -> bool` or a macro returning a `bool`.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!( till_colon, take_till!(|ch| ch == b':') );
///
///  let r = till_colon(&b"abcd:efgh"[..]);
///  assert_eq!(r, Ok((&b":efgh"[..], &b"abcd"[..])));
///  let r2 = till_colon(&b":abcdefgh"[..]); // empty match is allowed
///  assert_eq!(r2, Ok((&b":abcdefgh"[..], &b""[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_till (
  ($input:expr, $submac:ident!( $($args:tt)* )) => ({
    let res: $crate::IResult<_, _, _> = take_till!($input, (|c| $submac!(c, $($args)*)));
    res
  });
  ($input:expr, $f:expr) => (
    $crate::bytes::streaming::take_till($f)($input)
  );
);

/// `take_till1!(T -> bool) => &[T] -> IResult<&[T], &[T]>`
/// returns the longest non empty list of bytes until the provided function succeeds
///
/// The argument is either a function `&[T] -> bool` or a macro returning a `bool`.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, error::ErrorKind};
/// # fn main() {
///  named!( till1_colon, take_till1!(|ch| ch == b':') );
///
///  let r = till1_colon(&b"abcd:efgh"[..]);
///  assert_eq!(r, Ok((&b":efgh"[..], &b"abcd"[..])));
///
///  let r2 = till1_colon(&b":abcdefgh"[..]); // empty match is error
///  assert_eq!(r2, Err(Err::Error(error_position!(&b":abcdefgh"[..], ErrorKind::TakeTill1))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_till1 (
  ($input:expr, $submac:ident!( $($args:tt)* )) => ({
    let res: $crate::IResult<_, _, _> = take_till1!($input, (|c| $submac!(c, $($args)*)));
    res
  });
  ($input:expr, $f:expr) => (
    $crate::bytes::streaming::take_till1($f)($input)
  );
);

/// `take!(nb) => &[T] -> IResult<&[T], &[T]>`
/// generates a parser consuming the specified number of bytes
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  // Desmond parser
///  named!(take5, take!( 5 ) );
///
///  let a = b"abcdefgh";
///
///  assert_eq!(take5(&a[..]), Ok((&b"fgh"[..], &b"abcde"[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take (
  ($i:expr, $count:expr) => ({
    let c = $count as usize;
    let res: $crate::IResult<_,_,_> = $crate::bytes::streaming::take(c)($i);
    res
  });
);

/// `take_str!(nb) => &[T] -> IResult<&[T], &str>`
/// same as take! but returning a &str
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(take5( &[u8] ) -> &str, take_str!( 5 ) );
///
///  let a = b"abcdefgh";
///
///  assert_eq!(take5(&a[..]), Ok((&b"fgh"[..], "abcde")));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_str (
 ( $i:expr, $size:expr ) => (
    {
      let input: &[u8] = $i;

      map_res!(input, take!($size), $crate::lib::std::str::from_utf8)
    }
  );
);

/// `take_until!(tag) => &[T] -> IResult<&[T], &[T]>`
/// consumes data until it finds the specified tag.
///
/// The remainder still contains the tag.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(x, take_until!("foo"));
///  let r = x(&b"abcd foo efgh"[..]);
///  assert_eq!(r, Ok((&b"foo efgh"[..], &b"abcd "[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_until (
  ($i:expr, $substr:expr) => ({
    let res: $crate::IResult<_,_,_> = $crate::bytes::streaming::take_until($substr)($i);
    res
  });
);

/// `take_until1!(tag) => &[T] -> IResult<&[T], &[T]>`
/// consumes data (at least one byte) until it finds the specified tag
///
/// The remainder still contains the tag.
///
/// # Example
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!(x, take_until1!("foo"));
///
///  let r = x(&b"abcd foo efgh"[..]);
///
///  assert_eq!(r, Ok((&b"foo efgh"[..], &b"abcd "[..])));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_until1 (
  ($i:expr, $substr:expr) => (
    {
      use $crate::lib::std::result::Result::*;
      use $crate::lib::std::option::Option::*;
      use $crate::{Err,Needed,IResult,error::ErrorKind};
      use $crate::InputLength;
      use $crate::FindSubstring;
      use $crate::InputTake;
      let input = $i;

      let res: IResult<_,_> = match input.find_substring($substr) {
        None => {
          Err(Err::Incomplete(Needed::Size(1 + $substr.input_len())))
        },
        Some(0) => {
          let e = ErrorKind::TakeUntil;
          Err(Err::Error(error_position!($i, e)))
        },
        Some(index) => {
          Ok($i.take_split(index))
        },
      };
      res
    }
  );
);

#[cfg(test)]
mod tests {
  use crate::internal::{Err, Needed, IResult};
  #[cfg(feature = "alloc")]
  use crate::lib::std::string::String;
  #[cfg(feature = "alloc")]
  use crate::lib::std::vec::Vec;
  use crate::character::streaming::{alpha1 as alpha, alphanumeric1 as alphanumeric, digit1 as digit, hex_digit1 as hex_digit, multispace1 as multispace, oct_digit1 as oct_digit, space1 as space};
  use crate::error::ErrorKind;
  use crate::character::is_alphabetic;

  #[cfg(feature = "alloc")]
  macro_rules! one_of (
    ($i:expr, $inp: expr) => (
      {
        use $crate::Err;
        use $crate::Slice;
        use $crate::AsChar;
        use $crate::FindToken;
        use $crate::InputIter;

        match ($i).iter_elements().next().map(|c| {
          $inp.find_token(c)
        }) {
          None        => Err::<_,_>(Err::Incomplete(Needed::Size(1))),
          Some(false) => Err(Err::Error(error_position!($i, ErrorKind::OneOf))),
          //the unwrap should be safe here
          Some(true)  => Ok(($i.slice(1..), $i.iter_elements().next().unwrap().as_char()))
        }
      }
    );
  );

  #[test]
  fn is_a() {
    named!(a_or_b, is_a!(&b"ab"[..]));

    let a = &b"abcd"[..];
    assert_eq!(a_or_b(a), Ok((&b"cd"[..], &b"ab"[..])));

    let b = &b"bcde"[..];
    assert_eq!(a_or_b(b), Ok((&b"cde"[..], &b"b"[..])));

    let c = &b"cdef"[..];
    assert_eq!(a_or_b(c), Err(Err::Error(error_position!(c, ErrorKind::IsA))));

    let d = &b"bacdef"[..];
    assert_eq!(a_or_b(d), Ok((&b"cdef"[..], &b"ba"[..])));
  }

  #[test]
  fn is_not() {
    named!(a_or_b, is_not!(&b"ab"[..]));

    let a = &b"cdab"[..];
    assert_eq!(a_or_b(a), Ok((&b"ab"[..], &b"cd"[..])));

    let b = &b"cbde"[..];
    assert_eq!(a_or_b(b), Ok((&b"bde"[..], &b"c"[..])));

    let c = &b"abab"[..];
    assert_eq!(a_or_b(c), Err(Err::Error(error_position!(c, ErrorKind::IsNot))));

    let d = &b"cdefba"[..];
    assert_eq!(a_or_b(d), Ok((&b"ba"[..], &b"cdef"[..])));

    let e = &b"e"[..];
    assert_eq!(a_or_b(e), Err(Err::Incomplete(Needed::Size(1))));
  }

  #[cfg(feature = "alloc")]
  #[allow(unused_variables)]
  #[test]
  fn escaping() {
    named!(esc, escaped!(call!(alpha), '\\', one_of!("\"n\\")));
    assert_eq!(esc(&b"abcd;"[..]), Ok((&b";"[..], &b"abcd"[..])));
    assert_eq!(esc(&b"ab\\\"cd;"[..]), Ok((&b";"[..], &b"ab\\\"cd"[..])));
    assert_eq!(esc(&b"\\\"abcd;"[..]), Ok((&b";"[..], &b"\\\"abcd"[..])));
    assert_eq!(esc(&b"\\n;"[..]), Ok((&b";"[..], &b"\\n"[..])));
    assert_eq!(esc(&b"ab\\\"12"[..]), Ok((&b"12"[..], &b"ab\\\""[..])));
    assert_eq!(esc(&b"AB\\"[..]), Err(Err::Error(error_position!(&b"AB\\"[..], ErrorKind::Escaped))));
    assert_eq!(
      esc(&b"AB\\A"[..]),
      Err(Err::Error(error_node_position!(
        &b"AB\\A"[..],
        ErrorKind::Escaped,
        error_position!(&b"A"[..], ErrorKind::OneOf)
      )))
    );

    named!(esc2, escaped!(call!(digit), '\\', one_of!("\"n\\")));
    assert_eq!(esc2(&b"12\\nnn34"[..]), Ok((&b"nn34"[..], &b"12\\n"[..])));
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn escaping_str() {
    named!(esc<&str, &str>, escaped!(call!(alpha), '\\', one_of!("\"n\\")));
    assert_eq!(esc("abcd;"), Ok((";", "abcd")));
    assert_eq!(esc("ab\\\"cd;"), Ok((";", "ab\\\"cd")));
    assert_eq!(esc("\\\"abcd;"), Ok((";", "\\\"abcd")));
    assert_eq!(esc("\\n;"), Ok((";", "\\n")));
    assert_eq!(esc("ab\\\"12"), Ok(("12", "ab\\\"")));
    assert_eq!(esc("AB\\"), Err(Err::Error(error_position!("AB\\", ErrorKind::Escaped))));
    assert_eq!(
      esc("AB\\A"),
      Err(Err::Error(error_node_position!(
        "AB\\A",
        ErrorKind::Escaped,
        error_position!("A", ErrorKind::OneOf)
      )))
    );

    named!(esc2<&str, &str>, escaped!(call!(digit), '\\', one_of!("\"n\\")));
    assert_eq!(esc2("12\\nnn34"), Ok(("nn34", "12\\n")));

    named!(esc3<&str, &str>, escaped!(call!(alpha), '\u{241b}', one_of!("\"n")));
    assert_eq!(esc3("ab␛ncd;"), Ok((";", "ab␛ncd")));
  }

  #[cfg(feature = "alloc")]
  fn to_s(i: Vec<u8>) -> String {
    String::from_utf8_lossy(&i).into_owned()
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn escape_transform() {
    use crate::lib::std::str;

    named!(
      esc<String>,
      map!(
        escaped_transform!(
          alpha,
          '\\',
          alt!(
              tag!("\\")       => { |_| &b"\\"[..] }
            | tag!("\"")       => { |_| &b"\""[..] }
            | tag!("n")        => { |_| &b"\n"[..] }
          )
        ),
        to_s
      )
    );

    assert_eq!(esc(&b"abcd;"[..]), Ok((&b";"[..], String::from("abcd"))));
    assert_eq!(esc(&b"ab\\\"cd;"[..]), Ok((&b";"[..], String::from("ab\"cd"))));
    assert_eq!(esc(&b"\\\"abcd;"[..]), Ok((&b";"[..], String::from("\"abcd"))));
    assert_eq!(esc(&b"\\n;"[..]), Ok((&b";"[..], String::from("\n"))));
    assert_eq!(esc(&b"ab\\\"12"[..]), Ok((&b"12"[..], String::from("ab\""))));
    assert_eq!(esc(&b"AB\\"[..]), Err(Err::Error(error_position!(&b"\\"[..], ErrorKind::EscapedTransform))));
    assert_eq!(
      esc(&b"AB\\A"[..]),
      Err(Err::Error(error_node_position!(
        &b"AB\\A"[..],
        ErrorKind::EscapedTransform,
        error_position!(&b"A"[..], ErrorKind::Alt)
      )))
    );

    named!(
      esc2<String>,
      map!(
        escaped_transform!(
          call!(alpha),
          '&',
          alt!(
              tag!("egrave;") => { |_| str::as_bytes("è") }
            | tag!("agrave;") => { |_| str::as_bytes("à") }
          )
        ),
        to_s
      )
    );
    assert_eq!(esc2(&b"ab&egrave;DEF;"[..]), Ok((&b";"[..], String::from("abèDEF"))));
    assert_eq!(esc2(&b"ab&egrave;D&agrave;EF;"[..]), Ok((&b";"[..], String::from("abèDàEF"))));
  }

  #[cfg(feature = "std")]
  #[test]
  fn escape_transform_str() {
    named!(esc<&str, String>, escaped_transform!(alpha, '\\',
      alt!(
          tag!("\\")       => { |_| "\\" }
        | tag!("\"")       => { |_| "\"" }
        | tag!("n")        => { |_| "\n" }
      ))
    );

    assert_eq!(esc("abcd;"), Ok((";", String::from("abcd"))));
    assert_eq!(esc("ab\\\"cd;"), Ok((";", String::from("ab\"cd"))));
    assert_eq!(esc("\\\"abcd;"), Ok((";", String::from("\"abcd"))));
    assert_eq!(esc("\\n;"), Ok((";", String::from("\n"))));
    assert_eq!(esc("ab\\\"12"), Ok(("12", String::from("ab\""))));
    assert_eq!(esc("AB\\"), Err(Err::Error(error_position!("\\", ErrorKind::EscapedTransform))));
    assert_eq!(
      esc("AB\\A"),
      Err(Err::Error(error_node_position!(
        "AB\\A",
        ErrorKind::EscapedTransform,
        error_position!("A", ErrorKind::Alt)
      )))
    );

    named!(esc2<&str, String>, escaped_transform!(alpha, '&',
      alt!(
          tag!("egrave;") => { |_| "è" }
        | tag!("agrave;") => { |_| "à" }
      ))
    );
    assert_eq!(esc2("ab&egrave;DEF;"), Ok((";", String::from("abèDEF"))));
    assert_eq!(esc2("ab&egrave;D&agrave;EF;"), Ok((";", String::from("abèDàEF"))));

    named!(esc3<&str, String>, escaped_transform!(alpha, '␛',
      alt!(
        tag!("0") => { |_| "\0" } |
        tag!("n") => { |_| "\n" })));
    assert_eq!(esc3("a␛0bc␛n"), Ok(("", String::from("a\0bc\n"))));
  }

  #[test]
  fn take_str_test() {
    let a = b"omnomnom";

    let res: IResult<_,_,(&[u8], ErrorKind)> = take_str!(&a[..], 5u32);
    assert_eq!(res, Ok((&b"nom"[..], "omnom")));

    let res: IResult<_,_,(&[u8], ErrorKind)> = take_str!(&a[..], 9u32);
    assert_eq!(res, Err(Err::Incomplete(Needed::Size(9))));
  }

  #[test]
  fn take_until_incomplete() {
    named!(y, take_until!("end"));
    assert_eq!(y(&b"nd"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(y(&b"123"[..]), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(y(&b"123en"[..]), Err(Err::Incomplete(Needed::Size(3))));
  }

  #[test]
  fn take_until_incomplete_s() {
    named!(ys<&str, &str>, take_until!("end"));
    assert_eq!(ys("123en"), Err(Err::Incomplete(Needed::Size(3))));
  }

  #[test]
  fn recognize() {
    named!(x, recognize!(delimited!(tag!("<!--"), take!(5usize), tag!("-->"))));
    let r = x(&b"<!-- abc --> aaa"[..]);
    assert_eq!(r, Ok((&b" aaa"[..], &b"<!-- abc -->"[..])));

    let semicolon = &b";"[..];

    named!(ya, recognize!(alpha));
    let ra = ya(&b"abc;"[..]);
    assert_eq!(ra, Ok((semicolon, &b"abc"[..])));

    named!(yd, recognize!(digit));
    let rd = yd(&b"123;"[..]);
    assert_eq!(rd, Ok((semicolon, &b"123"[..])));

    named!(yhd, recognize!(hex_digit));
    let rhd = yhd(&b"123abcDEF;"[..]);
    assert_eq!(rhd, Ok((semicolon, &b"123abcDEF"[..])));

    named!(yod, recognize!(oct_digit));
    let rod = yod(&b"1234567;"[..]);
    assert_eq!(rod, Ok((semicolon, &b"1234567"[..])));

    named!(yan, recognize!(alphanumeric));
    let ran = yan(&b"123abc;"[..]);
    assert_eq!(ran, Ok((semicolon, &b"123abc"[..])));

    named!(ys, recognize!(space));
    let rs = ys(&b" \t;"[..]);
    assert_eq!(rs, Ok((semicolon, &b" \t"[..])));

    named!(yms, recognize!(multispace));
    let rms = yms(&b" \t\r\n;"[..]);
    assert_eq!(rms, Ok((semicolon, &b" \t\r\n"[..])));
  }

  #[test]
  fn take_while() {
    named!(f, take_while!(is_alphabetic));
    let a = b"";
    let b = b"abcd";
    let c = b"abcd123";
    let d = b"123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&b[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&c[..]), Ok((&d[..], &b[..])));
    assert_eq!(f(&d[..]), Ok((&d[..], &a[..])));
  }

  #[test]
  fn take_while1() {
    named!(f, take_while1!(is_alphabetic));
    let a = b"";
    let b = b"abcd";
    let c = b"abcd123";
    let d = b"123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&b[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&c[..]), Ok((&b"123"[..], &b[..])));
    assert_eq!(f(&d[..]), Err(Err::Error(error_position!(&d[..], ErrorKind::TakeWhile1))));
  }

  #[test]
  fn take_while_m_n() {
    named!(x, take_while_m_n!(2, 4, is_alphabetic));
    let a = b"";
    let b = b"a";
    let c = b"abc";
    let d = b"abc123";
    let e = b"abcde";
    let f = b"123";

    assert_eq!(x(&a[..]), Err(Err::Incomplete(Needed::Size(2))));
    assert_eq!(x(&b[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(x(&c[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(x(&d[..]), Ok((&b"123"[..], &c[..])));
    assert_eq!(x(&e[..]), Ok((&b"e"[..], &b"abcd"[..])));
    assert_eq!(x(&f[..]), Err(Err::Error(error_position!(&f[..], ErrorKind::TakeWhileMN))));
  }

  #[test]
  fn take_till() {

    named!(f, take_till!(is_alphabetic));
    let a = b"";
    let b = b"abcd";
    let c = b"123abcd";
    let d = b"123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&b[..]), Ok((&b"abcd"[..], &b""[..])));
    assert_eq!(f(&c[..]), Ok((&b"abcd"[..], &b"123"[..])));
    assert_eq!(f(&d[..]), Err(Err::Incomplete(Needed::Size(1))));
  }

  #[test]
  fn take_till1() {

    named!(f, take_till1!(is_alphabetic));
    let a = b"";
    let b = b"abcd";
    let c = b"123abcd";
    let d = b"123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f(&b[..]), Err(Err::Error(error_position!(&b[..], ErrorKind::TakeTill1))));
    assert_eq!(f(&c[..]), Ok((&b"abcd"[..], &b"123"[..])));
    assert_eq!(f(&d[..]), Err(Err::Incomplete(Needed::Size(1))));
  }

  #[test]
  fn take_while_utf8() {
    named!(f<&str,&str>, take_while!(|c:char| { c != '點' }));

    assert_eq!(f(""), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f("abcd"), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f("abcd點"), Ok(("點", "abcd")));
    assert_eq!(f("abcd點a"), Ok(("點a", "abcd")));

    named!(g<&str,&str>, take_while!(|c:char| { c == '點' }));

    assert_eq!(g(""), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(g("點abcd"), Ok(("abcd", "點")));
    assert_eq!(g("點點點a"), Ok(("a", "點點點")));
  }

  #[test]
  fn take_till_utf8() {
    named!(f<&str,&str>, take_till!(|c:char| { c == '點' }));

    assert_eq!(f(""), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f("abcd"), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(f("abcd點"), Ok(("點", "abcd")));
    assert_eq!(f("abcd點a"), Ok(("點a", "abcd")));

    named!(g<&str,&str>, take_till!(|c:char| { c != '點' }));

    assert_eq!(g(""), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(g("點abcd"), Ok(("abcd", "點")));
    assert_eq!(g("點點點a"), Ok(("a", "點點點")));
  }

  #[test]
  fn take_utf8() {
    named!(f<&str,&str>, take!(3));

    assert_eq!(f(""), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(f("ab"), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(f("點"), Err(Err::Incomplete(Needed::Size(3))));
    assert_eq!(f("ab點cd"), Ok(("cd", "ab點")));
    assert_eq!(f("a點bcd"), Ok(("cd", "a點b")));
    assert_eq!(f("a點b"), Ok(("", "a點b")));

    named!(g<&str,&str>, take_while!(|c:char| { c == '點' }));

    assert_eq!(g(""), Err(Err::Incomplete(Needed::Size(1))));
    assert_eq!(g("點abcd"), Ok(("abcd", "點")));
    assert_eq!(g("點點點a"), Ok(("a", "點點點")));
  }

  #[test]
  fn take_while_m_n_utf8() {
    named!(parser<&str, &str>, take_while_m_n!(1, 1, |c| c == 'A' || c == '😃'));
    assert_eq!(parser("A!"), Ok(("!", "A")));
    assert_eq!(parser("😃!"), Ok(("!", "😃")));
  }

  #[test]
  fn take_while_m_n_utf8_full_match() {
    named!(parser<&str, &str>, take_while_m_n!(1, 1, |c: char| c.is_alphabetic()));
    assert_eq!(parser("øn"), Ok(("n", "ø")));
  }

  #[cfg(nightly)]
  use test::Bencher;

  #[cfg(nightly)]
  #[bench]
  fn take_while_bench(b: &mut Bencher) {

    named!(f, take_while!(is_alphabetic));
    b.iter(|| f(&b"abcdefghijklABCDEejfrfrjgro12aa"[..]));
  }

  #[test]
  #[cfg(feature = "std")]
  fn recognize_take_while() {
    use crate::character::is_alphanumeric;
    named!(x, take_while!(is_alphanumeric));
    named!(y, recognize!(x));
    assert_eq!(x(&b"ab."[..]), Ok((&b"."[..], &b"ab"[..])));
    println!("X: {:?}", x(&b"ab"[..]));
    assert_eq!(y(&b"ab."[..]), Ok((&b"."[..], &b"ab"[..])));
  }

  #[test]
  fn length_bytes() {
    use crate::number::streaming::le_u8;
    named!(x, length_data!(le_u8));
    assert_eq!(x(b"\x02..>>"), Ok((&b">>"[..], &b".."[..])));
    assert_eq!(x(b"\x02.."), Ok((&[][..], &b".."[..])));
    assert_eq!(x(b"\x02."), Err(Err::Incomplete(Needed::Size(2))));
    assert_eq!(x(b"\x02"), Err(Err::Incomplete(Needed::Size(2))));

    named!(y, do_parse!(tag!("magic") >> b: length_data!(le_u8) >> (b)));
    assert_eq!(y(b"magic\x02..>>"), Ok((&b">>"[..], &b".."[..])));
    assert_eq!(y(b"magic\x02.."), Ok((&[][..], &b".."[..])));
    assert_eq!(y(b"magic\x02."), Err(Err::Incomplete(Needed::Size(2))));
    assert_eq!(y(b"magic\x02"), Err(Err::Incomplete(Needed::Size(2))));
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn case_insensitive() {
    named!(test, tag_no_case!("ABcd"));
    assert_eq!(test(&b"aBCdefgh"[..]), Ok((&b"efgh"[..], &b"aBCd"[..])));
    assert_eq!(test(&b"abcdefgh"[..]), Ok((&b"efgh"[..], &b"abcd"[..])));
    assert_eq!(test(&b"ABCDefgh"[..]), Ok((&b"efgh"[..], &b"ABCD"[..])));
    assert_eq!(test(&b"ab"[..]), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(test(&b"Hello"[..]), Err(Err::Error(error_position!(&b"Hello"[..], ErrorKind::Tag))));
    assert_eq!(test(&b"Hel"[..]), Err(Err::Error(error_position!(&b"Hel"[..], ErrorKind::Tag))));

    named!(test2<&str, &str>, tag_no_case!("ABcd"));
    assert_eq!(test2("aBCdefgh"), Ok(("efgh", "aBCd")));
    assert_eq!(test2("abcdefgh"), Ok(("efgh", "abcd")));
    assert_eq!(test2("ABCDefgh"), Ok(("efgh", "ABCD")));
    assert_eq!(test2("ab"), Err(Err::Incomplete(Needed::Size(4))));
    assert_eq!(test2("Hello"), Err(Err::Error(error_position!(&"Hello"[..], ErrorKind::Tag))));
    assert_eq!(test2("Hel"), Err(Err::Error(error_position!(&"Hel"[..], ErrorKind::Tag))));
  }

  #[test]
  fn tag_fixed_size_array() {
    named!(test, tag!([0x42]));
    named!(test2, tag!(&[0x42]));
    let input = [0x42, 0x00];
    assert_eq!(test(&input), Ok((&b"\x00"[..], &b"\x42"[..])));
    assert_eq!(test2(&input), Ok((&b"\x00"[..], &b"\x42"[..])));
  }
}
