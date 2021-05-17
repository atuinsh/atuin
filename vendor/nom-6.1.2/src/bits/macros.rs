//! Bit level parsers and combinators
//!
//! Bit parsing is handled by tweaking the input in most macros.
//! In byte level parsing, the input is generally a `&[u8]` passed from combinator
//! to combinator as the slices are manipulated.
//!
//! Bit parsers take a `(&[u8], usize)` as input. The first part of the tuple is a byte slice,
//! the second part is a bit offset in the first byte of the slice.
//!
//! By passing a pair like this, we can leverage most of the existing combinators, and avoid
//! transforming the whole slice to a vector of booleans. This should make it easy
//! to see a byte slice as a bit stream, and parse code points of arbitrary bit length.
//!

/// Transforms its byte slice input into a bit stream for the underlying parser. This allows the
/// given bit stream parser to work on a byte slice input.
///
/// Signature:
/// `bits!( parser ) => ( &[u8], (&[u8], usize) -> IResult<(&[u8], usize), T> ) -> IResult<&[u8], T>`
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// # fn main() {
///  named!( take_4_bits<u8>, bits!( take_bits!( 4u8 ) ) );
///
///  let input = vec![0xAB, 0xCD, 0xEF, 0x12];
///  let sl    = &input[..];
///
///  assert_eq!(take_4_bits( sl ), Ok( (&sl[1..], 0xA) ));
///  assert_eq!(take_4_bits( &b""[..] ), Err(Err::Incomplete(Needed::new(1))));
/// # }
#[macro_export(local_inner_macros)]
macro_rules! bits (
  ($i:expr, $submac:ident!( $($args:tt)* )) => ({
    $crate::bits::bitsc($i, move |i| { $submac!(i, $($args)*) })
  });
  ($i:expr, $f:expr) => (
    bits!($i, call!($f))
  );
);

/// Counterpart to `bits`, `bytes!` transforms its bit stream input into a byte slice for the underlying
/// parser, allowing byte-slice parsers to work on bit streams.
///
/// Signature:
/// `bytes!( parser ) => ( (&[u8], usize), &[u8] -> IResult<&[u8], T> ) -> IResult<(&[u8], usize), T>`,
///
/// A partial byte remaining in the input will be ignored and the given parser will start parsing
/// at the next full byte.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # use nom::combinator::rest;
/// # use nom::error::{Error, ErrorKind};
/// # fn main() {
///
/// named!( parse<(u8, u8, &[u8])>,  bits!( tuple!(
///    take_bits!(4u8),
///    take_bits!(8u8),
///    bytes!(rest::<_, Error<_>>)
/// )));
///
///  let input = &[0xde, 0xad, 0xbe, 0xaf];
///
///  assert_eq!(parse( input ), Ok(( &[][..], (0xd, 0xea, &[0xbe, 0xaf][..]) )));
/// # }
#[macro_export(local_inner_macros)]
macro_rules! bytes (
  ($i:expr, $submac:ident!( $($args:tt)* )) => ({
    $crate::bits::bytesc($i, move |i| { $submac!(i, $($args)*) })
  });
  ($i:expr, $f:expr) => (
    bytes!($i, call!($f))
  );
);

/// Consumes the specified number of bits and returns them as the specified type.
///
/// Signature:
/// `take_bits!(type, count) => ( (&[T], usize), U, usize) -> IResult<(&[T], usize), U>`
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
/// named!(bits_pair<(&[u8], usize), (u8, u8)>, pair!( take_bits!(4u8), take_bits!(4u8) ) );
/// named!( take_pair<(u8, u8)>, bits!( bits_pair ) );
///
/// let input = vec![0xAB, 0xCD, 0xEF];
/// let sl    = &input[..];
///
/// assert_eq!(take_pair( sl ),       Ok((&sl[1..], (0xA, 0xB))) );
/// assert_eq!(take_pair( &sl[1..] ), Ok((&sl[2..], (0xC, 0xD))) );
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! take_bits (
  ($i:expr, $count:expr) => (
    {
      let res: $crate::IResult<_, _> = $crate::bits::streaming::take($count)($i);
      res
    }
  );
);

/// Matches the given bit pattern.
///
/// Signature:
/// `tag_bits!(type, count, pattern) => ( (&[T], usize), U, usize, U) -> IResult<(&[T], usize), U>`
///
/// The caller must specify the number of bits to consume. The matched value is included in the
/// result on success.
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///  named!( take_a<u8>, bits!( tag_bits!(4usize, 0xA) ) );
///
///  let input = vec![0xAB, 0xCD, 0xEF];
///  let sl    = &input[..];
///
///  assert_eq!(take_a( sl ),       Ok((&sl[1..], 0xA)) );
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! tag_bits (
  ($i:expr, $count:expr, $p: expr) => (
    {
      let res: $crate::IResult<_, _> = $crate::bits::streaming::tag($p, $count)($i);
      res
    }
  )
);

#[cfg(test)]
mod tests {
  use crate::error::ErrorKind;
  use crate::internal::{Err, IResult, Needed};
  use crate::lib::std::ops::{AddAssign, Shl, Shr};

  #[test]
  fn take_bits() {
    let input = [0b10_10_10_10, 0b11_11_00_00, 0b00_11_00_11];
    let sl = &input[..];

    assert_eq!(take_bits!((sl, 0), 0u8), Ok(((sl, 0), 0)));
    assert_eq!(take_bits!((sl, 0), 8u8), Ok(((&sl[1..], 0), 170)));
    assert_eq!(take_bits!((sl, 0), 3u8), Ok(((&sl[0..], 3), 5)));
    assert_eq!(take_bits!((sl, 0), 6u8), Ok(((&sl[0..], 6), 42)));
    assert_eq!(take_bits!((sl, 1), 1u8), Ok(((&sl[0..], 2), 0)));
    assert_eq!(take_bits!((sl, 1), 2u8), Ok(((&sl[0..], 3), 1)));
    assert_eq!(take_bits!((sl, 1), 3u8), Ok(((&sl[0..], 4), 2)));
    assert_eq!(take_bits!((sl, 6), 3u8), Ok(((&sl[1..], 1), 5)));
    assert_eq!(take_bits!((sl, 0), 10u8), Ok(((&sl[1..], 2), 683)));
    assert_eq!(take_bits!((sl, 0), 8u8), Ok(((&sl[1..], 0), 170)));
    assert_eq!(take_bits!((sl, 6), 10u8), Ok(((&sl[2..], 0), 752)));
    assert_eq!(take_bits!((sl, 6), 11u8), Ok(((&sl[2..], 1), 1504)));
    assert_eq!(take_bits!((sl, 0), 20u8), Ok(((&sl[2..], 4), 700_163)));
    assert_eq!(take_bits!((sl, 4), 20u8), Ok(((&sl[3..], 0), 716_851)));
    let r: IResult<_, u32> = take_bits!((sl, 4), 22u8);
    assert_eq!(r, Err(Err::Incomplete(Needed::new(22))));
  }

  #[test]
  fn tag_bits() {
    let input = [0b10_10_10_10, 0b11_11_00_00, 0b00_11_00_11];
    let sl = &input[..];

    assert_eq!(tag_bits!((sl, 0), 3u8, 0b101), Ok(((&sl[0..], 3), 5)));
    assert_eq!(tag_bits!((sl, 0), 4u8, 0b1010), Ok(((&sl[0..], 4), 10)));
  }

  named!(ch<(&[u8],usize),(u8,u8)>,
    do_parse!(
      tag_bits!(3u8, 0b101) >>
      x: take_bits!(4u8)    >>
      y: take_bits!(5u8)    >>
      (x,y)
    )
  );

  #[test]
  fn chain_bits() {
    let input = [0b10_10_10_10, 0b11_11_00_00, 0b00_11_00_11];
    let sl = &input[..];
    assert_eq!(ch((&input[..], 0)), Ok(((&sl[1..], 4), (5, 15))));
    assert_eq!(ch((&input[..], 4)), Ok(((&sl[2..], 0), (7, 16))));
    assert_eq!(ch((&input[..1], 0)), Err(Err::Incomplete(Needed::new(5))));
  }

  named!(ch_bytes<(u8, u8)>, bits!(ch));
  #[test]
  fn bits_to_bytes() {
    let input = [0b10_10_10_10, 0b11_11_00_00, 0b00_11_00_11];
    assert_eq!(ch_bytes(&input[..]), Ok((&input[2..], (5, 15))));
    assert_eq!(ch_bytes(&input[..1]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(
      ch_bytes(&input[1..]),
      Err(Err::Error(error_position!(&input[1..], ErrorKind::TagBits)))
    );
  }

  named!(
    bits_bytes_bs,
    bits!(bytes!(
      crate::combinator::rest::<_, crate::error::Error<&[u8]>>
    ))
  );
  #[test]
  fn bits_bytes() {
    let input = [0b10_10_10_10];
    assert_eq!(
      bits_bytes_bs(&input[..]),
      Ok((&[][..], &[0b10_10_10_10][..]))
    );
  }

  #[derive(PartialEq, Debug)]
  struct FakeUint(u32);

  impl AddAssign for FakeUint {
    fn add_assign(&mut self, other: FakeUint) {
      *self = FakeUint(self.0 + other.0);
    }
  }

  impl Shr<usize> for FakeUint {
    type Output = FakeUint;

    fn shr(self, shift: usize) -> FakeUint {
      FakeUint(self.0 >> shift)
    }
  }

  impl Shl<usize> for FakeUint {
    type Output = FakeUint;

    fn shl(self, shift: usize) -> FakeUint {
      FakeUint(self.0 << shift)
    }
  }

  impl From<u8> for FakeUint {
    fn from(i: u8) -> FakeUint {
      FakeUint(u32::from(i))
    }
  }

  #[test]
  fn non_privitive_type() {
    let input = [0b10_10_10_10, 0b11_11_00_00, 0b00_11_00_11];
    let sl = &input[..];

    assert_eq!(
      take_bits!((sl, 0), 20u8),
      Ok(((&sl[2..], 4), FakeUint(700_163)))
    );
    assert_eq!(
      take_bits!((sl, 4), 20u8),
      Ok(((&sl[3..], 0), FakeUint(716_851)))
    );
    let r3: IResult<_, FakeUint> = take_bits!((sl, 4), 22u8);
    assert_eq!(r3, Err(Err::Incomplete(Needed::new(22))));
  }
}
