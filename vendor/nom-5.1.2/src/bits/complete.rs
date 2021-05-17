//! bit level parsers
//!

use crate::error::{ErrorKind, ParseError};
use crate::internal::{Err, IResult};
use crate::lib::std::ops::{AddAssign, RangeFrom, Shl, Shr, Div};
use crate::traits::{InputIter, InputLength, Slice, ToUsize};

/// generates a parser taking `count` bits
pub fn take<I, O, C, E: ParseError<(I, usize)>>(count: C) -> impl Fn((I, usize)) -> IResult<(I, usize), O, E>
where
  I: Slice<RangeFrom<usize>> + InputIter<Item = u8> + InputLength,
  C: ToUsize,
  O: From<u8> + AddAssign + Shl<usize, Output = O> + Shr<usize, Output = O>,
{
  let count = count.to_usize();
  move |(input, bit_offset): (I, usize)| {
    if count == 0 {
      Ok(((input, bit_offset), 0u8.into()))
    } else {
      let cnt = (count + bit_offset).div(8);
      if input.input_len() * 8 < count + bit_offset {
        Err(Err::Error(E::from_error_kind((input, bit_offset), ErrorKind::Eof)))
      } else {
        let mut acc:O             = (0 as u8).into();
        let mut offset: usize     = bit_offset;
        let mut remaining: usize  = count;
        let mut end_offset: usize = 0;

        for byte in input.iter_elements().take(cnt + 1) {
          if remaining == 0 {
            break;
          }
          let val: O = if offset == 0 {
            byte.into()
          } else {
            ((byte << offset) as u8 >> offset).into()
          };

          if remaining < 8 - offset {
            acc += val >> (8 - offset - remaining);
            end_offset = remaining + offset;
            break;
          } else {
            acc += val << (remaining - (8 - offset));
            remaining -= 8 - offset;
            offset = 0;
          }
        }
        Ok(( (input.slice(cnt..), end_offset) , acc))
      }
    }
  }
}

/// generates a parser taking `count` bits and comparing them to `pattern`
pub fn tag<I, O, C, E: ParseError<(I, usize)>>(pattern: O, count: C) -> impl Fn((I, usize)) -> IResult<(I, usize), O, E>
where
  I: Slice<RangeFrom<usize>> + InputIter<Item = u8> + InputLength + Clone,
  C: ToUsize,
  O: From<u8> + AddAssign + Shl<usize, Output = O> + Shr<usize, Output = O> + PartialEq,
{
  let count = count.to_usize();
  move |input: (I, usize)| {
    let inp = input.clone();

    take(count)(input).and_then(|(i, o)| {
      if pattern == o {
        Ok((i, o))
      } else {
        Err(Err::Error(error_position!(inp, ErrorKind::TagBits)))
      }
    })
  }
}
