#![cfg_attr(feature = "cargo-clippy", allow(unreadable_literal))]
#![cfg(target_pointer_width = "64")]

#[macro_use]
extern crate nom;

use nom::{Err, Needed};
#[cfg(feature = "alloc")]
use nom::number::streaming::be_u64;

// Parser definition

// We request a length that would trigger an overflow if computing consumed + requested
named!(parser01<&[u8],()>,
    do_parse!(
        hdr: take!(1) >>
        data: take!(18446744073709551615) >>
        ({
          let _ = hdr;
          let _ = data;
          ()
        })
    )
);

// We request a length that would trigger an overflow if computing consumed + requested
named!(parser02<&[u8],(&[u8],&[u8])>,
    tuple!(take!(1),take!(18446744073709551615))
);

#[test]
fn overflow_incomplete_do_parse() {
  assert_eq!(
    parser01(&b"3"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551615)))
  );
}

#[test]
fn overflow_incomplete_tuple() {
  assert_eq!(
    parser02(&b"3"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551615)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_length_bytes() {
  named!(multi<&[u8], Vec<&[u8]> >, many0!( length_data!(be_u64) ) );

  // Trigger an overflow in length_data
  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xff\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551615)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_many0() {
  named!(multi<&[u8], Vec<&[u8]> >, many0!( length_data!(be_u64) ) );

  // Trigger an overflow in many0
  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xef\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551599)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_many1() {
  named!(multi<&[u8], Vec<&[u8]> >, many1!( length_data!(be_u64) ) );

  // Trigger an overflow in many1
  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xef\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551599)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_many_till() {
  named!(multi<&[u8], (Vec<&[u8]>, &[u8]) >, many_till!( length_data!(be_u64), tag!("abc") ) );

  // Trigger an overflow in many_till
  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xef\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551599)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_many_m_n() {
  named!(multi<&[u8], Vec<&[u8]> >, many_m_n!(2, 4, length_data!(be_u64) ) );

  // Trigger an overflow in many_m_n
  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xef\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551599)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_count() {
  named!(counter<&[u8], Vec<&[u8]> >, count!( length_data!(be_u64), 2 ) );

  assert_eq!(
    counter(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xef\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551599)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_length_count() {
  use nom::number::streaming::be_u8;
  named!(multi<&[u8], Vec<&[u8]> >, length_count!( be_u8, length_data!(be_u64) ) );

  assert_eq!(
    multi(&b"\x04\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xee\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551598)))
  );
}

#[test]
#[cfg(feature = "alloc")]
fn overflow_incomplete_length_data() {
  named!(multi<&[u8], Vec<&[u8]> >, many0!( length_data!(be_u64) ) );

  assert_eq!(
    multi(&b"\x00\x00\x00\x00\x00\x00\x00\x01\xaa\xff\xff\xff\xff\xff\xff\xff\xff\xaa"[..]),
    Err(Err::Incomplete(Needed::Size(18446744073709551615)))
  );
}
