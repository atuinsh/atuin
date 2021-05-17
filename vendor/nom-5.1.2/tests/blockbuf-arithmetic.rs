/*
#[macro_use]
extern crate nom;
extern crate bytes;

use nom::{Compare,CompareResult,InputLength,InputIter,Slice,HexDisplay};

use std::str;
use std::str::FromStr;
use bytes::{Buf,MutBuf};
use bytes::buf::{BlockBuf,BlockBufCursor};
use std::ops::{Range,RangeTo,RangeFrom,RangeFull};
use std::iter::{Enumerate,Iterator};
use std::fmt;
use std::cmp::{min,PartialEq};

#[derive(Clone,Copy)]
#[repr(C)]
pub struct BlockSlice<'a> {
  buf: &'a BlockBuf,
  start: usize,
  end:   usize,
}

impl<'a> BlockSlice<'a> {
  fn cursor(&self) -> WrapCursor<'a> {
    let mut cur = self.buf.buf();
    cur.advance(self.start);
    WrapCursor {
      cursor: cur,
      length: self.end - self.start,
    }
  }
}

impl<'a> fmt::Debug for BlockSlice<'a> {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "BlockSlice {{ start: {}, end: {}, data:\n{}\n}}", self.start, self.end, self.buf.bytes().unwrap_or(&b""[..]).to_hex(16))
  }
}

impl<'a> PartialEq for BlockSlice<'a> {
  fn eq(&self, other: &BlockSlice<'a>) -> bool {
    let bufs = (self.buf as *const BlockBuf) == (other.buf as *const BlockBuf);
    self.start == other.start && self.end == other.end && bufs
  }
}

impl<'a> Slice<Range<usize>> for BlockSlice<'a> {
  fn slice(&self, range:Range<usize>) -> Self {
    BlockSlice {
      buf:   self.buf,
      start: self.start + range.start,
      //FIXME: check for valid end here
      end:   self.start + range.end,
    }
  }
}

impl<'a> Slice<RangeTo<usize>> for BlockSlice<'a> {
  fn slice(&self, range:RangeTo<usize>) -> Self {
    self.slice(0..range.end)
  }
}

impl<'a> Slice<RangeFrom<usize>> for BlockSlice<'a> {
  fn slice(&self, range:RangeFrom<usize>) -> Self {
    self.slice(range.start..self.end - self.start)
  }
}

impl<'a> Slice<RangeFull> for BlockSlice<'a> {
  fn slice(&self, _:RangeFull) -> Self {
    BlockSlice {
      buf:   self.buf,
      start: self.start,
      end:   self.end,
    }
  }
}


impl<'a> InputIter for BlockSlice<'a> {
    type Item     = u8;
    type RawItem  = u8;
    type Iter     = Enumerate<WrapCursor<'a>>;
    type IterElem = WrapCursor<'a>;

    fn iter_indices(&self)  -> Self::Iter {
      self.cursor().enumerate()
    }
    fn iter_elements(&self) -> Self::IterElem {
      self.cursor()
    }
    fn position<P>(&self, predicate: P) -> Option<usize> where P: Fn(Self::RawItem) -> bool {
      self.cursor().position(|b| predicate(b))
    }
    fn slice_index(&self, count:usize) -> Option<usize> {
      if self.end - self.start >= count {
        Some(count)
      } else {
        None
      }
    }
}


impl<'a> InputLength for BlockSlice<'a> {
  fn input_len(&self) -> usize {
    self.end - self.start
  }
}

impl<'a,'b> Compare<&'b[u8]> for BlockSlice<'a> {
  fn compare(&self, t: &'b[u8]) -> CompareResult {
    let len     = self.end - self.start;
    let blen    = t.len();
    let m       = if len < blen { len } else { blen };
    let reduced = self.slice(..m);
    let b       = &t[..m];

    for (a,b) in reduced.cursor().zip(b.iter()) {
      if a != *b {
        return CompareResult::Error;
      }
    }
    if m < blen {
      CompareResult::Incomplete
    } else {
      CompareResult::Ok
    }
  }


  #[inline(always)]
  fn compare_no_case(&self, t: &'b[u8]) -> CompareResult {
    let len     = self.end - self.start;
    let blen    = t.len();
    let m       = if len < blen { len } else { blen };
    let reduced = self.slice(..m);
    let other   = &t[..m];

    if !reduced.cursor().zip(other).all(|(a, b)| {
      match (a,*b) {
        (0...64, 0...64) | (91...96, 91...96) | (123...255, 123...255) => a == *b,
        (65...90, 65...90) | (97...122, 97...122) | (65...90, 97...122 ) |(97...122, 65...90) => {
          a & 0b01000000 == *b & 0b01000000
        }
        _ => false
      }
    }) {
      CompareResult::Error
    } else if m < blen {
      CompareResult::Incomplete
    } else {
      CompareResult::Ok
    }
  }
}

impl<'a,'b> Compare<&'b str> for BlockSlice<'a> {
  fn compare(&self, t: &'b str) -> CompareResult {
    self.compare(str::as_bytes(t))
  }
  fn compare_no_case(&self, t: &'b str) -> CompareResult {
    self.compare_no_case(str::as_bytes(t))
  }
}

//Wrapper to implement Iterator on BlockBufCursor
pub struct WrapCursor<'a> {
  pub cursor: BlockBufCursor<'a>,
  pub length: usize,
}

impl<'a> Iterator for WrapCursor<'a> {
  type Item = u8;
  fn next(&mut self) -> Option<u8> {
    //println!("NEXT: length={}, remaining={}", self.length, self.cursor.remaining());
    if min(self.length, self.cursor.remaining()) > 0 {
      self.length -=1;
      Some(self.cursor.read_u8())
    } else {
      None
    }
  }
}

//Reimplement eat_separator instead of fixing iterators
#[macro_export]
macro_rules! block_eat_separator (
  ($i:expr, $arr:expr) => (
    {
      use nom::{InputLength,InputIter,Slice};
      if ($i).input_len() == 0 {
        Ok(($i, ($i).slice(0..0)))
      } else {
        match ($i).iter_indices().position(|(_, item)| {
          for (_,c) in ($arr).iter_indices() {
            if *c == item { return false; }
          }
          true
        }) {
          Some(index) => {
            Ok((($i).slice(index..), ($i).slice(..index)))
          },
          None => {
            Ok((($i).slice(($i).input_len()..), $i))
          }
        }
      }
    }
  )
);

#[macro_export]
macro_rules! block_named (
  ($name:ident, $submac:ident!( $($args:tt)* )) => (
    fn $name<'a>( i: BlockSlice<'a> ) -> nom::IResult<BlockSlice<'a>, BlockSlice<'a>, u32> {
      $submac!(i, $($args)*)
    }
  );
  ($name:ident<$o:ty>, $submac:ident!( $($args:tt)* )) => (
    fn $name<'a>( i: BlockSlice<'a> ) -> nom::IResult<BlockSlice<'a>, $o, u32> {
      $submac!(i, $($args)*)
    }
  );
);

block_named!(sp, block_eat_separator!(&b" \t\r\n"[..]));

macro_rules! block_ws (
  ($i:expr, $($args:tt)*) => (
    {
      sep!($i, sp, $($args)*)
    }
  )
);

block_named!(digit, is_a!("0123456789"));

block_named!(parens<i64>, block_ws!(delimited!( tag!("("), expr, tag!(")") )) );


block_named!(factor<i64>, alt!(
      map_res!(
        block_ws!(digit),
        to_i64
    )
  | parens
  )
);

block_named!(term <i64>, do_parse!(
    init: factor >>
    res:  fold_many0!(
        pair!(alt!(tag!("*") | tag!("/")), factor),
        init,
        |acc, (op, val): (BlockSlice, i64)| {
            if (op.cursor().next().unwrap() as char) == '*' { acc * val } else { acc / val }
        }
    ) >>
    (res)
  )
);

block_named!(expr <i64>, do_parse!(
    init: term >>
    res:  fold_many0!(
        pair!(alt!(tag!("+") | tag!("-")), term),
        init,
        |acc, (op, val): (BlockSlice, i64)| {
            if (op.cursor().next().unwrap() as char) == '+' { acc + val } else { acc - val }
        }
    ) >>
    (res)
  )
);


fn blockbuf_from(input: &[u8]) -> BlockBuf {
  let mut b = BlockBuf::new(2, 100);
  b.copy_from(input);
  b
}


fn sl<'a>(input: &'a BlockBuf) -> BlockSlice<'a> {
  BlockSlice {
    buf: input,
    start: 0,
    end:   input.len(),
  }
}

fn to_i64<'a>(input: BlockSlice<'a>) -> Result<i64, ()> {
  let v: Vec<u8> = input.cursor().collect();

  match str::from_utf8(&v) {
    Err(_) => Err(()),
    Ok(s) => match FromStr::from_str(s) {
      Err(_) => Err(()),
      Ok(i)  => Ok(i)
    }
  }
}

#[test]
fn factor_test() {
  let a = blockbuf_from(&b"3"[..]);
  println!("calculated: {:?}", factor(sl(&a)));
}

#[test]
fn parens_test() {
  let input1 = blockbuf_from(&b" 2* (  3 + 4 ) "[..]);
  println!("calculated 1: {:?}", expr(sl(&input1)));
  let input2 = blockbuf_from(&b"  2*2 / ( 5 - 1) + 3"[..]);
  println!("calculated 2: {:?}", expr(sl(&input2)));
}
*/
