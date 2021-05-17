//! bit level parsers
//!

#[macro_use]
mod macros;

pub mod streaming;
pub mod complete;

use crate::error::{ParseError, ErrorKind};
use crate::internal::{Err, IResult, Needed};
use crate::lib::std::ops::RangeFrom;
use crate::traits::{Slice, ErrorConvert};


/// Converts a byte-level input to a bit-level input, for consumption by a parser that uses bits.
///
/// Afterwards, the input is converted back to a byte-level parser, with any remaining bits thrown
/// away.
///
/// # Example
/// ```ignore
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// use nom::bits::bits;
/// use nom::bits::complete::take;
///
/// fn take_4_bits(input: &[u8]) -> IResult<&[u8], u64> {
///   bits(take::<_, _, _, (_, _)>(4usize))(input)
/// }
///
/// let input = vec![0xAB, 0xCD, 0xEF, 0x12];
/// let sl    = &input[..];
///
/// assert_eq!(take_4_bits( sl ), Ok( (&sl[1..], 0xA) ));
/// ```
pub fn bits<I, O, E1: ParseError<(I, usize)>+ErrorConvert<E2>, E2: ParseError<I>, P>(parser: P) -> impl Fn(I) -> IResult<I, O, E2>
where
  I: Slice<RangeFrom<usize>>,
  P: Fn((I, usize)) -> IResult<(I, usize), O, E1>,
{
  move |input: I| match parser((input, 0)) {
    Ok(((rest, offset), res)) => {
      let byte_index = offset / 8 + if offset % 8 == 0 { 0 } else { 1 };
      Ok((rest.slice(byte_index..), res))
    }
    Err(Err::Incomplete(n)) => Err(Err::Incomplete(n.map(|u| u / 8 + 1))),
    Err(Err::Error(e)) => Err(Err::Error(e.convert())),
    Err(Err::Failure(e)) => Err(Err::Failure(e.convert())),
  }
}

#[doc(hidden)]
pub fn bitsc<I, O, E1: ParseError<(I, usize)>+ErrorConvert<E2>, E2: ParseError<I>, P>(input: I, parser: P) -> IResult<I, O, E2>
where
  I: Slice<RangeFrom<usize>>,
  P: Fn((I, usize)) -> IResult<(I, usize), O, E1>,
{
  bits(parser)(input)
}

/// Counterpart to bits, bytes transforms its bit stream input into a byte slice for the underlying
/// parser, allowing byte-slice parsers to work on bit streams.
///
/// A partial byte remaining in the input will be ignored and the given parser will start parsing
/// at the next full byte.
///
/// ```ignore
/// # #[macro_use] extern crate nom;
/// # use nom::IResult;
/// # use nom::combinator::rest;
/// # use nom::sequence::tuple;
/// use nom::bits::{bits, bytes, streaming::take_bits};
///
/// fn parse(input: &[u8]) -> IResult<&[u8], (u8, u8, &[u8])> {
///   bits(tuple((
///     take_bits(4usize),
///     take_bits(8usize),
///     bytes(rest)
///   )))(input)
/// }
///
/// let input = &[0xde, 0xad, 0xbe, 0xaf];
///
/// assert_eq!(parse( input ), Ok(( &[][..], (0xd, 0xea, &[0xbe, 0xaf][..]) )));
/// ```
pub fn bytes<I, O, E1: ParseError<I>+ErrorConvert<E2>, E2: ParseError<(I, usize)>, P>(parser: P) -> impl Fn((I, usize)) -> IResult<(I, usize), O, E2>
where
  I: Slice<RangeFrom<usize>> + Clone,
  P: Fn(I) -> IResult<I, O, E1>,
{
  move |(input, offset): (I, usize)| {
    let inner = if offset % 8 != 0 {
      input.slice((1 + offset / 8)..)
    } else {
      input.slice((offset / 8)..)
    };
    let i = (input.clone(), offset);
    match parser(inner) {
      Ok((rest, res)) => Ok(((rest, 0), res)),
      Err(Err::Incomplete(Needed::Unknown)) => Err(Err::Incomplete(Needed::Unknown)),
      Err(Err::Incomplete(Needed::Size(sz))) => Err(match sz.checked_mul(8) {
        Some(v) => Err::Incomplete(Needed::Size(v)),
        None => Err::Failure(E2::from_error_kind(i, ErrorKind::TooLarge)),
      }),
      Err(Err::Error(e)) => Err(Err::Error(e.convert())),
      Err(Err::Failure(e)) => Err(Err::Failure(e.convert())),
    }
  }
}

#[doc(hidden)]
pub fn bytesc<I, O, E1: ParseError<I>+ErrorConvert<E2>, E2: ParseError<(I, usize)>, P>(input: (I, usize), parser: P) -> IResult<(I, usize), O, E2>
where
  I: Slice<RangeFrom<usize>> + Clone,
  P: Fn(I) -> IResult<I, O, E1>,
{
  bytes(parser)(input)
}
