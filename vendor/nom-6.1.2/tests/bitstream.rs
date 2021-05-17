#![cfg(feature = "bitvec")]

use bitvec::prelude::*;
use nom::{
  bytes::complete::{tag, take},
  combinator::map,
  IResult,
};

#[test]
fn parse_bitstream() {
  let data = [0xA5u8, 0x69, 0xF0, 0xC3];
  let bits = data.view_bits::<Msb0>();

  fn parser(bits: &BitSlice<Msb0, u8>) -> IResult<&BitSlice<Msb0, u8>, &BitSlice<Msb0, u8>> {
    tag(bits![1, 0, 1, 0])(bits)
  }

  assert_eq!(parser(bits), Ok((&bits[4..], &bits[..4])));
}

#[test]
fn parse_bitstream_map() {
  let data = [0b1000_0000];
  let bits = data.view_bits::<Msb0>();

  fn parser(bits: &BitSlice<Msb0, u8>) -> IResult<&BitSlice<Msb0, u8>, bool> {
    map(take(1_u8), |val: &BitSlice<Msb0, u8>| val[0])(bits)
  }

  assert_eq!(parser(bits), Ok((&bits[1..], true)));
}
