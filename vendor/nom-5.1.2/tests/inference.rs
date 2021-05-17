//! test type inference issues in parsee compilation
//#![feature(trace_macros)]
#![allow(dead_code)]
#![allow(unused_comparisons)]
#![allow(unused_variables)]
#![allow(unused_imports)]

#[macro_use]
extern crate nom;

use std::str;
use nom::character::{streaming::alpha1 as alpha, is_digit};

// issue #617
named!(multi<&[u8], () >, fold_many0!( take_while1!( is_digit ), (), |_, _| {}));

// issue #561
#[cfg(feature = "alloc")]
named!(
  value<Vec<Vec<&str>>>,
  do_parse!(
    first_line: map_res!(is_not!("\n"), std::str::from_utf8)
      >> rest:
        many_m_n!(
          0,
          1,
          separated_list!(
            tag!("\n\t"),
            map_res!(take_while!(call!(|c| c != b'\n')), std::str::from_utf8)
          )
        ) >> (rest)
  )
);

// issue #534
#[cfg(feature = "alloc")]
fn wrap_suffix(input: &Option<Vec<&[u8]>>) -> Option<String> {
  if input.is_some() {
    // I've tried both of the lines below individually and get the same error.
    Some("hello".to_string())
  //Some(str::from_utf8(u).expect("Found invalid UTF-8").to_string())
  } else {
    None
  }
}

#[cfg(feature = "alloc")]
named!(parse_suffix<&[u8],Option<String>>,do_parse!(
  u: opt!(many1!(alt!(
    complete!(tag!("%")) | complete!(tag!("#"))  | complete!(tag!("@")) | complete!(alpha)
  ))) >>
  (wrap_suffix(&u))
));
