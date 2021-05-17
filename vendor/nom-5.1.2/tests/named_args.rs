#[macro_use]
extern crate nom;

use nom::{
  branch::alt,
  sequence::{delimited, pair, preceded},
  character::complete::{digit1 as digit, space0 as space},
  bytes::complete::tag
};

// Parser definition

use std::str;
use std::str::FromStr;

use self::Operator::*;

enum Operator {
  Slash,
  Star,
}

impl Operator {
  fn to_str(&self) -> &'static str {
    match *self {
      Slash => "/",
      Star => "*",
    }
  }
}

// Parse the specified `Operator`.
named_args!(operator(op: Operator) <&[u8], &[u8]>,
    call!(tag(op.to_str()))
);

// We parse any expr surrounded by the tags `open_tag` and `close_tag`, ignoring all whitespaces around those
named_args!(brackets<'a>(open_tag: &str, close_tag: &str) <&'a[u8], i64>,
  call!(delimited(
    space,
    delimited(tag(open_tag), preceded(space, expr), preceded(space, tag(close_tag))),
    space
  ))
);

fn byte_slice_to_str<'a>(s: &'a[u8]) -> Result<&'a str, str::Utf8Error> {
  str::from_utf8(s)
}

// We transform an integer string into a i64, ignoring surrounding whitespaces
// We look for a digit suite, and try to convert it.
// If either str::from_utf8 or FromStr::from_str fail,
// we fallback to the brackets parser defined above
named!(factor<&[u8], i64>, alt!(
    map_res!(
      map_res!(
        call!(delimited(space, digit, space)),
        byte_slice_to_str
      ),
      FromStr::from_str
    )
  | call!(brackets, "(", ")")
  )
);

// We read an initial factor and for each time we find
// a * or / operator followed by another factor, we do
// the math by folding everything
named!(term <&[u8], i64>, do_parse!(
    init: factor >>
    res:  fold_many0!(
        pair!(alt!(call!(operator, Star) | call!(operator, Slash)), factor),
        init,
        |acc, (op, val): (&[u8], i64)| {
            if (op[0] as char) == '*' { acc * val } else { acc / val }
        }
    ) >>
    (res)
  )
);

named!(expr <&[u8], i64>, do_parse!(
    init: term >>
    res:  fold_many0!(
        call!(pair(alt((tag("+"), tag("-"))), term)),
        init,
        |acc, (op, val): (&[u8], i64)| {
            if (op[0] as char) == '+' { acc + val } else { acc - val }
        }
    ) >>
    (res)
  )
);

#[test]
fn factor_test() {
  assert_eq!(
    factor(&b"3"[..]),
    Ok((&b""[..], 3))
  );
  assert_eq!(
    factor(&b" 12"[..]),
    Ok((&b""[..], 12))
  );
  assert_eq!(
    factor(&b"537  "[..]),
    Ok((&b""[..], 537))
  );
  assert_eq!(
    factor(&b"  24   "[..]),
    Ok((&b""[..], 24))
  );
}

#[test]
fn term_test() {
  assert_eq!(
    term(&b" 12 *2 /  3"[..]),
    Ok((&b""[..], 8))
  );
  assert_eq!(
    term(&b" 2* 3  *2 *2 /  3"[..]),
    Ok((&b""[..], 8))
  );
  assert_eq!(
    term(&b" 48 /  3/2"[..]),
    Ok((&b""[..], 8))
  );
}

#[test]
fn expr_test() {
  assert_eq!(
    expr(&b" 1 +  2 "[..]),
    Ok((&b""[..], 3))
  );
  assert_eq!(
    expr(&b" 12 + 6 - 4+  3"[..]),
    Ok((&b""[..], 17))
  );
  assert_eq!(
    expr(&b" 1 + 2*3 + 4"[..]),
    Ok((&b""[..], 11))
  );
}

#[test]
fn parens_test() {
  assert_eq!(
    expr(&b" (  2 )"[..]),
    Ok((&b""[..], 2))
  );
  assert_eq!(
    expr(&b" 2* (  3 + 4 ) "[..]),
    Ok((&b""[..], 14))
  );
  assert_eq!(
    expr(&b"  2*2 / ( 5 - 1) + 3"[..]),
    Ok((&b""[..], 4))
  );
}
