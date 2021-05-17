//! Traits input types have to implement to work with nom combinators
//!
use crate::internal::{Err, IResult, Needed};
use crate::error::{ParseError, ErrorKind};
use crate::lib::std::ops::{Range, RangeFrom, RangeFull, RangeTo};
use crate::lib::std::iter::Enumerate;
use crate::lib::std::slice::Iter;
use crate::lib::std::iter::Map;
use crate::lib::std::str::Chars;
use crate::lib::std::str::CharIndices;
use crate::lib::std::str::FromStr;
use crate::lib::std::str::from_utf8;
use memchr;

#[cfg(feature = "alloc")]
use crate::lib::std::string::String;
#[cfg(feature = "alloc")]
use crate::lib::std::vec::Vec;

/// abstract method to calculate the input length
pub trait InputLength {
  /// calculates the input length, as indicated by its name,
  /// and the name of the trait itself
  fn input_len(&self) -> usize;
}

impl<'a, T> InputLength for &'a [T] {
  #[inline]
  fn input_len(&self) -> usize {
    self.len()
  }
}

impl<'a> InputLength for &'a str {
  #[inline]
  fn input_len(&self) -> usize {
    self.len()
  }
}

impl<'a> InputLength for (&'a [u8], usize) {
  #[inline]
  fn input_len(&self) -> usize {
    //println!("bit input length for ({:?}, {}):", self.0, self.1);
    //println!("-> {}", self.0.len() * 8 - self.1);
    self.0.len() * 8 - self.1
  }
}

/// useful functions to calculate the offset between slices and show a hexdump of a slice
pub trait Offset {
  /// offset between the first byte of self and the first byte of the argument
  fn offset(&self, second: &Self) -> usize;
}

impl Offset for [u8] {
  fn offset(&self, second: &Self) -> usize {
    let fst = self.as_ptr();
    let snd = second.as_ptr();

    snd as usize - fst as usize
  }
}

impl<'a> Offset for &'a [u8] {
  fn offset(&self, second: &Self) -> usize {
    let fst = self.as_ptr();
    let snd = second.as_ptr();

    snd as usize - fst as usize
  }
}

impl Offset for str {
  fn offset(&self, second: &Self) -> usize {
    let fst = self.as_ptr();
    let snd = second.as_ptr();

    snd as usize - fst as usize
  }
}

impl<'a> Offset for &'a str {
  fn offset(&self, second: &Self) -> usize {
    let fst = self.as_ptr();
    let snd = second.as_ptr();

    snd as usize - fst as usize
  }
}

/// Helper trait for types that can be viewed as a byte slice
pub trait AsBytes {
  /// casts the input type to a byte slice
  fn as_bytes(&self) -> &[u8];
}

impl<'a> AsBytes for &'a str {
  #[inline(always)]
  fn as_bytes(&self) -> &[u8] {
    <str as AsBytes>::as_bytes(self)
  }
}

impl AsBytes for str {
  #[inline(always)]
  fn as_bytes(&self) -> &[u8] {
    self.as_ref()
  }
}

impl<'a> AsBytes for &'a [u8] {
  #[inline(always)]
  fn as_bytes(&self) -> &[u8] {
    *self
  }
}

impl AsBytes for [u8] {
  #[inline(always)]
  fn as_bytes(&self) -> &[u8] {
    self
  }
}

macro_rules! as_bytes_array_impls {
  ($($N:expr)+) => {
    $(
      impl<'a> AsBytes for &'a [u8; $N] {
        #[inline(always)]
        fn as_bytes(&self) -> &[u8] {
          *self
        }
      }

      impl AsBytes for [u8; $N] {
        #[inline(always)]
        fn as_bytes(&self) -> &[u8] {
          self
        }
      }
    )+
  };
}

as_bytes_array_impls! {
     0  1  2  3  4  5  6  7  8  9
    10 11 12 13 14 15 16 17 18 19
    20 21 22 23 24 25 26 27 28 29
    30 31 32
}

/// transforms common types to a char for basic token parsing
pub trait AsChar {
  /// makes a char from self
  fn as_char(self) -> char;

  /// tests that self is an alphabetic character
  ///
  /// warning: for `&str` it recognizes alphabetic
  /// characters outside of the 52 ASCII letters
  fn is_alpha(self) -> bool;

  /// tests that self is an alphabetic character
  /// or a decimal digit
  fn is_alphanum(self) -> bool;
  /// tests that self is a decimal digit
  fn is_dec_digit(self) -> bool;
  /// tests that self is an hex digit
  fn is_hex_digit(self) -> bool;
  /// tests that self is an octal digit
  fn is_oct_digit(self) -> bool;
  /// gets the len in bytes for self
  fn len(self) -> usize;
}

impl AsChar for u8 {
  #[inline]
  fn as_char(self) -> char {
    self as char
  }
  #[inline]
  fn is_alpha(self) -> bool {
    (self >= 0x41 && self <= 0x5A) || (self >= 0x61 && self <= 0x7A)
  }
  #[inline]
  fn is_alphanum(self) -> bool {
    self.is_alpha() || self.is_dec_digit()
  }
  #[inline]
  fn is_dec_digit(self) -> bool {
    self >= 0x30 && self <= 0x39
  }
  #[inline]
  fn is_hex_digit(self) -> bool {
    (self >= 0x30 && self <= 0x39) || (self >= 0x41 && self <= 0x46) || (self >= 0x61 && self <= 0x66)
  }
  #[inline]
  fn is_oct_digit(self) -> bool {
    self >= 0x30 && self <= 0x37
  }
  #[inline]
  fn len(self) -> usize {
    1
  }
}
impl<'a> AsChar for &'a u8 {
  #[inline]
  fn as_char(self) -> char {
    *self as char
  }
  #[inline]
  fn is_alpha(self) -> bool {
    (*self >= 0x41 && *self <= 0x5A) || (*self >= 0x61 && *self <= 0x7A)
  }
  #[inline]
  fn is_alphanum(self) -> bool {
    self.is_alpha() || self.is_dec_digit()
  }
  #[inline]
  fn is_dec_digit(self) -> bool {
    *self >= 0x30 && *self <= 0x39
  }
  #[inline]
  fn is_hex_digit(self) -> bool {
    (*self >= 0x30 && *self <= 0x39) || (*self >= 0x41 && *self <= 0x46) || (*self >= 0x61 && *self <= 0x66)
  }
  #[inline]
  fn is_oct_digit(self) -> bool {
    *self >= 0x30 && *self <= 0x37
  }
  #[inline]
  fn len(self) -> usize {
    1
  }
}

impl AsChar for char {
  #[inline]
  fn as_char(self) -> char {
    self
  }
  #[inline]
  fn is_alpha(self) -> bool {
    self.is_ascii_alphabetic()
  }
  #[inline]
  fn is_alphanum(self) -> bool {
    self.is_alpha() || self.is_dec_digit()
  }
  #[inline]
  fn is_dec_digit(self) -> bool {
    self.is_ascii_digit()
  }
  #[inline]
  fn is_hex_digit(self) -> bool {
    self.is_ascii_hexdigit()
  }
  #[inline]
  fn is_oct_digit(self) -> bool {
    self.is_digit(8)
  }
  #[inline]
  fn len(self) -> usize {
    self.len_utf8()
  }
}

impl<'a> AsChar for &'a char {
  #[inline]
  fn as_char(self) -> char {
    *self
  }
  #[inline]
  fn is_alpha(self) -> bool {
    self.is_ascii_alphabetic()
  }
  #[inline]
  fn is_alphanum(self) -> bool {
    self.is_alpha() || self.is_dec_digit()
  }
  #[inline]
  fn is_dec_digit(self) -> bool {
    self.is_ascii_digit()
  }
  #[inline]
  fn is_hex_digit(self) -> bool {
    self.is_ascii_hexdigit()
  }
  #[inline]
  fn is_oct_digit(self) -> bool {
    self.is_digit(8)
  }
  #[inline]
  fn len(self) -> usize {
    self.len_utf8()
  }
}

/// abstracts common iteration operations on the input type
pub trait InputIter {
  /// the current input type is a sequence of that `Item` type.
  ///
  /// example: `u8` for `&[u8]` or `char` for &str`
  type Item;
  /// an iterator over the input type, producing the item and its position
  /// for use with [Slice]. If we're iterating over `&str`, the position
  /// corresponds to the byte index of the character
  type Iter: Iterator<Item = (usize, Self::Item)>;

  /// an iterator over the input type, producing the item
  type IterElem: Iterator<Item = Self::Item>;

  /// returns an iterator over the elements and their byte offsets
  fn iter_indices(&self) -> Self::Iter;
  /// returns an iterator over the elements
  fn iter_elements(&self) -> Self::IterElem;
  /// finds the byte position of the element
  fn position<P>(&self, predicate: P) -> Option<usize>
  where
    P: Fn(Self::Item) -> bool;
  /// get the byte offset from the element's position in the stream
  fn slice_index(&self, count: usize) -> Option<usize>;
}

/// abstracts slicing operations
pub trait InputTake: Sized {
  /// returns a slice of `count` bytes. panics if count > length
  fn take(&self, count: usize) -> Self;
  /// split the stream at the `count` byte offset. panics if count > length
  fn take_split(&self, count: usize) -> (Self, Self);
}

fn star(r_u8: &u8) -> u8 {
  *r_u8
}

impl<'a> InputIter for &'a [u8] {
  type Item = u8;
  type Iter = Enumerate<Self::IterElem>;
  type IterElem = Map<Iter<'a, Self::Item>, fn(&u8) -> u8>;

  #[inline]
  fn iter_indices(&self) -> Self::Iter {
    self.iter_elements().enumerate()
  }
  #[inline]
  fn iter_elements(&self) -> Self::IterElem {
    self.iter().map(star)
  }
  #[inline]
  fn position<P>(&self, predicate: P) -> Option<usize>
  where
    P: Fn(Self::Item) -> bool,
  {
    self.iter().position(|b| predicate(*b))
  }
  #[inline]
  fn slice_index(&self, count: usize) -> Option<usize> {
    if self.len() >= count {
      Some(count)
    } else {
      None
    }
  }
}

impl<'a> InputTake for &'a [u8] {
  #[inline]
  fn take(&self, count: usize) -> Self {
    &self[0..count]
  }
  #[inline]
  fn take_split(&self, count: usize) -> (Self, Self) {
    let (prefix, suffix) = self.split_at(count);
    (suffix, prefix)
  }
}

impl<'a> InputIter for &'a str {
  type Item = char;
  type Iter = CharIndices<'a>;
  type IterElem = Chars<'a>;
  #[inline]
  fn iter_indices(&self) -> Self::Iter {
    self.char_indices()
  }
  #[inline]
  fn iter_elements(&self) -> Self::IterElem {
    self.chars()
  }
  fn position<P>(&self, predicate: P) -> Option<usize>
  where
    P: Fn(Self::Item) -> bool,
  {
    for (o, c) in self.char_indices() {
      if predicate(c) {
        return Some(o);
      }
    }
    None
  }
  #[inline]
  fn slice_index(&self, count: usize) -> Option<usize> {
    let mut cnt = 0;
    for (index, _) in self.char_indices() {
      if cnt == count {
        return Some(index);
      }
      cnt += 1;
    }
    if cnt == count {
      return Some(self.len());
    }
    None
  }
}

impl<'a> InputTake for &'a str {
  #[inline]
  fn take(&self, count: usize) -> Self {
    &self[..count]
  }

  // return byte index
  #[inline]
  fn take_split(&self, count: usize) -> (Self, Self) {
    (&self[count..], &self[..count])
  }
}

/// Dummy trait used for default implementations (currently only used for `InputTakeAtPosition`).
///
/// When implementing a custom input type, it is possible to use directly the
/// default implementation: if the input type implements `InputLength`, `InputIter`,
/// `InputTake` and `Clone`, you can implement `UnspecializedInput` and get
/// a default version of `InputTakeAtPosition`.
///
/// For performance reasons, you might want to write a custom implementation of
/// `InputTakeAtPosition` (like the one for `&[u8]`).
pub trait UnspecializedInput {}

/// methods to take as much input as possible until the provided function returns true for the current element
///
/// a large part of nom's basic parsers are built using this trait
pub trait InputTakeAtPosition: Sized {
  /// the current input type is a sequence of that `Item` type.
  ///
  /// example: `u8` for `&[u8]` or `char` for &str`
  type Item;

  /// looks for the first element of the input type for which the condition returns true,
  /// and returns the input up to this position
  ///
  /// *streaming version*: if no element is found matching the condition, this will return `Incomplete`
  fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool;

  /// looks for the first element of the input type for which the condition returns true
  /// and returns the input up to this position
  ///
  /// fails if the produced slice is empty
  ///
  /// *streaming version*: if no element is found matching the condition, this will return `Incomplete`
  fn split_at_position1<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool;

  /// looks for the first element of the input type for which the condition returns true,
  /// and returns the input up to this position
  ///
  /// *complete version*: if no element is found matching the condition, this will return the whole input
  fn split_at_position_complete<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool;

  /// looks for the first element of the input type for which the condition returns true
  /// and returns the input up to this position
  ///
  /// fails if the produced slice is empty
  ///
  /// *complete version*: if no element is found matching the condition, this will return the whole input
  fn split_at_position1_complete<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool;
}

impl<T: InputLength + InputIter + InputTake + Clone + UnspecializedInput> InputTakeAtPosition for T {
  type Item = <T as InputIter>::Item;

  fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match self.position(predicate) {
      Some(n) => Ok(self.take_split(n)),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position1<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match self.position(predicate) {
      Some(0) => Err(Err::Error(E::from_error_kind(self.clone(), e))),
      Some(n) => Ok(self.take_split(n)),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position_complete<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match self.split_at_position(predicate) {
      Err(Err::Incomplete(_)) => Ok(self.take_split(self.input_len())),
      res => res,
    }
  }

  fn split_at_position1_complete<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match self.split_at_position1(predicate, e) {
      Err(Err::Incomplete(_)) => if self.input_len() == 0 {
        Err(Err::Error(E::from_error_kind(self.clone(), e)))
      } else {
        Ok(self.take_split(self.input_len()))
      }
      res => res,
    }
  }
}

impl<'a> InputTakeAtPosition for &'a [u8] {
  type Item = u8;

  fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match (0..self.len()).find(|b| predicate(self[*b])) {
      Some(i) => Ok((&self[i..], &self[..i])),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position1<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match (0..self.len()).find(|b| predicate(self[*b])) {
      Some(0) => Err(Err::Error(E::from_error_kind(self, e))),
      Some(i) => Ok((&self[i..], &self[..i])),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position_complete<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match (0..self.len()).find(|b| predicate(self[*b])) {
      Some(i) => Ok((&self[i..], &self[..i])),
      None => Ok(self.take_split(self.input_len())),
    }
  }

  fn split_at_position1_complete<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match (0..self.len()).find(|b| predicate(self[*b])) {
      Some(0) => Err(Err::Error(E::from_error_kind(self, e))),
      Some(i) => Ok((&self[i..], &self[..i])),
      None => {
        if self.len() == 0 {
          Err(Err::Error(E::from_error_kind(self, e)))
        } else {
          Ok(self.take_split(self.input_len()))
        }
      },
    }
  }
}

impl<'a> InputTakeAtPosition for &'a str {
  type Item = char;

  fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match self.find(predicate) {
      Some(i) => Ok((&self[i..], &self[..i])),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position1<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
  where
    P: Fn(Self::Item) -> bool,
  {
    match self.find(predicate) {
      Some(0) => Err(Err::Error(E::from_error_kind(self, e))),
      Some(i) => Ok((&self[i..], &self[..i])),
      None => Err(Err::Incomplete(Needed::Size(1))),
    }
  }

  fn split_at_position_complete<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match self.find(predicate) {
      Some(i) => Ok((&self[i..], &self[..i])),
      None =>  Ok(self.take_split(self.input_len()))
    }
  }

  fn split_at_position1_complete<P, E: ParseError<Self>>(&self, predicate: P, e: ErrorKind) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
    match self.find(predicate) {
      Some(0) => Err(Err::Error(E::from_error_kind(self, e))),
      Some(i) => Ok((&self[i..], &self[..i])),
      None => {
        if self.len() == 0 {
          Err(Err::Error(E::from_error_kind(self, e)))
        } else {
          Ok(self.take_split(self.input_len()))
        }
      },
    }
  }
}

/// indicates wether a comparison was successful, an error, or
/// if more data was needed
#[derive(Debug, PartialEq)]
pub enum CompareResult {
  /// comparison was successful
  Ok,
  /// we need more data to be sure
  Incomplete,
  /// comparison failed
  Error,
}

/// abstracts comparison operations
pub trait Compare<T> {
  /// compares self to another value for equality
  fn compare(&self, t: T) -> CompareResult;
  /// compares self to another value for equality
  /// independently of the case.
  ///
  /// warning: for `&str`, the comparison is done
  /// by lowercasing both strings and comparing
  /// the result. This is a temporary solution until
  /// a better one appears
  fn compare_no_case(&self, t: T) -> CompareResult;
}

impl<'a, 'b> Compare<&'b [u8]> for &'a [u8] {
  #[inline(always)]
  fn compare(&self, t: &'b [u8]) -> CompareResult {
    let pos = self.iter().zip(t.iter()).position(|(a, b)| a != b);

    match pos {
      Some(_) => CompareResult::Error,
      None => {
        if self.len() >= t.len() {
          CompareResult::Ok
        } else {
          CompareResult::Incomplete
        }
      }
    }

    /*
    let len = self.len();
    let blen = t.len();
    let m = if len < blen { len } else { blen };
    let reduced = &self[..m];
    let b = &t[..m];

    if reduced != b {
      CompareResult::Error
    } else if m < blen {
      CompareResult::Incomplete
    } else {
      CompareResult::Ok
    }
    */
  }

  #[inline(always)]
  fn compare_no_case(&self, t: &'b [u8]) -> CompareResult {
    let len = self.len();
    let blen = t.len();
    let m = if len < blen { len } else { blen };
    let reduced = &self[..m];
    let other = &t[..m];

    if !reduced.iter().zip(other).all(|(a, b)| match (*a, *b) {
      (0..=64, 0..=64) | (91..=96, 91..=96) | (123..=255, 123..=255) => a == b,
      (65..=90, 65..=90) | (97..=122, 97..=122) | (65..=90, 97..=122) | (97..=122, 65..=90) => *a | 0b00_10_00_00 == *b | 0b00_10_00_00,
      _ => false,
    }) {
      CompareResult::Error
    } else if m < blen {
      CompareResult::Incomplete
    } else {
      CompareResult::Ok
    }
  }
}

impl<'a, 'b> Compare<&'b str> for &'a [u8] {
  #[inline(always)]
  fn compare(&self, t: &'b str) -> CompareResult {
    self.compare(AsBytes::as_bytes(t))
  }
  #[inline(always)]
  fn compare_no_case(&self, t: &'b str) -> CompareResult {
    self.compare_no_case(AsBytes::as_bytes(t))
  }
}

impl<'a, 'b> Compare<&'b str> for &'a str {
  #[inline(always)]
  fn compare(&self, t: &'b str) -> CompareResult {
    let pos = self.chars().zip(t.chars()).position(|(a, b)| a != b);

    match pos {
      Some(_) => CompareResult::Error,
      None => {
        if self.len() >= t.len() {
          CompareResult::Ok
        } else {
          CompareResult::Incomplete
        }
      }
    }
  }

  //FIXME: this version is too simple and does not use the current locale
  #[inline(always)]
  fn compare_no_case(&self, t: &'b str) -> CompareResult {
    let pos = self
      .chars()
      .zip(t.chars())
      .position(|(a, b)| a.to_lowercase().zip(b.to_lowercase()).any(|(a, b)| a != b));

    match pos {
      Some(_) => CompareResult::Error,
      None => {
        if self.len() >= t.len() {
          CompareResult::Ok
        } else {
          CompareResult::Incomplete
        }
      }
    }
  }
}

/// look for a token in self
pub trait FindToken<T> {
  /// returns true if self contains the token
  fn find_token(&self, token: T) -> bool;
}

impl<'a> FindToken<u8> for &'a [u8] {
  fn find_token(&self, token: u8) -> bool {
    memchr::memchr(token, self).is_some()
  }
}

impl<'a> FindToken<u8> for &'a str {
  fn find_token(&self, token: u8) -> bool {
    self.as_bytes().find_token(token)
  }
}

impl<'a, 'b> FindToken<&'a u8> for &'b [u8] {
  fn find_token(&self, token: &u8) -> bool {
    memchr::memchr(*token, self).is_some()
  }
}

impl<'a, 'b> FindToken<&'a u8> for &'b str {
  fn find_token(&self, token: &u8) -> bool {
    self.as_bytes().find_token(token)
  }
}

impl<'a> FindToken<char> for &'a [u8] {
  fn find_token(&self, token: char) -> bool {
    for i in self.iter() {
      if token as u8 == *i {
        return true;
      }
    }
    false
  }
}

impl<'a> FindToken<char> for &'a str {
  fn find_token(&self, token: char) -> bool {
    for i in self.chars() {
      if token == i {
        return true;
      }
    }
    false
  }
}

/// look for a substring in self
pub trait FindSubstring<T> {
  /// returns the byte position of the substring if it is found
  fn find_substring(&self, substr: T) -> Option<usize>;
}

impl<'a, 'b> FindSubstring<&'b [u8]> for &'a [u8] {
  fn find_substring(&self, substr: &'b [u8]) -> Option<usize> {
    let substr_len = substr.len();

    if substr_len == 0 {
      // an empty substring is found at position 0
      // This matches the behavior of str.find("").
      Some(0)
    } else if substr_len == 1 {
      memchr::memchr(substr[0], self)
    } else if substr_len > self.len() {
      None
    } else {
      let max = self.len() - substr_len;
      let mut offset = 0;
      let mut haystack = &self[..];

      while let Some(position) = memchr::memchr(substr[0], haystack) {
        offset += position;

        if offset > max {
          return None;
        }

        if &haystack[position..position + substr_len] == substr {
          return Some(offset);
        }

        haystack = &haystack[position + 1..];
        offset += 1;
      }

      None
    }
  }
}

impl<'a, 'b> FindSubstring<&'b str> for &'a [u8] {
  fn find_substring(&self, substr: &'b str) -> Option<usize> {
    self.find_substring(AsBytes::as_bytes(substr))
  }
}

impl<'a, 'b> FindSubstring<&'b str> for &'a str {
  //returns byte index
  fn find_substring(&self, substr: &'b str) -> Option<usize> {
    self.find(substr)
  }
}

/// used to integrate str's parse() method
pub trait ParseTo<R> {
  /// succeeds if `parse()` succeeded. The byte slice implementation
  /// will first convert it to a &str, then apply the `parse()` function
  fn parse_to(&self) -> Option<R>;
}

impl<'a, R: FromStr> ParseTo<R> for &'a [u8] {
  fn parse_to(&self) -> Option<R> {
    from_utf8(self).ok().and_then(|s| s.parse().ok())
  }
}

impl<'a, R: FromStr> ParseTo<R> for &'a str {
  fn parse_to(&self) -> Option<R> {
    self.parse().ok()
  }
}

/// slicing operations using ranges
///
/// this trait is loosely based on
/// `Index`, but can actually return
/// something else than a `&[T]` or `&str`
pub trait Slice<R> {
  /// slices self according to the range argument
  fn slice(&self, range: R) -> Self;
}

macro_rules! impl_fn_slice {
    ( $ty:ty ) => {
        fn slice(&self, range:$ty) -> Self {
            &self[range]
        }
    }
}

macro_rules! slice_range_impl {
    ( [ $for_type:ident ], $ty:ty ) => {
        impl<'a, $for_type> Slice<$ty> for &'a [$for_type] {
            impl_fn_slice!( $ty );
        }
    };
    ( $for_type:ty, $ty:ty ) => {
        impl<'a> Slice<$ty> for &'a $for_type {
            impl_fn_slice!( $ty );
        }
    }
}

macro_rules! slice_ranges_impl {
    ( [ $for_type:ident ] ) => {
        slice_range_impl! {[$for_type], Range<usize>}
        slice_range_impl! {[$for_type], RangeTo<usize>}
        slice_range_impl! {[$for_type], RangeFrom<usize>}
        slice_range_impl! {[$for_type], RangeFull}
    };
    ( $for_type:ty ) => {
        slice_range_impl! {$for_type, Range<usize>}
        slice_range_impl! {$for_type, RangeTo<usize>}
        slice_range_impl! {$for_type, RangeFrom<usize>}
        slice_range_impl! {$for_type, RangeFull}
    }
}

slice_ranges_impl! {str}
slice_ranges_impl! {[T]}

macro_rules! array_impls {
  ($($N:expr)+) => {
    $(
      impl InputLength for [u8; $N] {
        #[inline]
        fn input_len(&self) -> usize {
          self.len()
        }
      }

      impl<'a> InputLength for &'a [u8; $N] {
        #[inline]
        fn input_len(&self) -> usize {
          self.len()
        }
      }

      impl<'a> Compare<[u8; $N]> for &'a [u8] {
        #[inline(always)]
        fn compare(&self, t: [u8; $N]) -> CompareResult {
          self.compare(&t[..])
        }

        #[inline(always)]
        fn compare_no_case(&self, t: [u8;$N]) -> CompareResult {
          self.compare_no_case(&t[..])
        }
      }

      impl<'a,'b> Compare<&'b [u8; $N]> for &'a [u8] {
        #[inline(always)]
        fn compare(&self, t: &'b [u8; $N]) -> CompareResult {
          self.compare(&t[..])
        }

        #[inline(always)]
        fn compare_no_case(&self, t: &'b [u8;$N]) -> CompareResult {
          self.compare_no_case(&t[..])
        }
      }

      impl FindToken<u8> for [u8; $N] {
        fn find_token(&self, token: u8) -> bool {
          memchr::memchr(token, &self[..]).is_some()
        }
      }

      impl<'a> FindToken<&'a u8> for [u8; $N] {
        fn find_token(&self, token: &u8) -> bool {
          memchr::memchr(*token, &self[..]).is_some()
        }
      }
    )+
  };
}

array_impls! {
     0  1  2  3  4  5  6  7  8  9
    10 11 12 13 14 15 16 17 18 19
    20 21 22 23 24 25 26 27 28 29
    30 31 32
}

/// abstracts something which can extend an `Extend`
/// used to build modified input slices in `escaped_transform`
pub trait ExtendInto {

  /// the current input type is a sequence of that `Item` type.
  ///
  /// example: `u8` for `&[u8]` or `char` for &str`
  type Item;

  /// the type that will be produced
  type Extender: Extend<Self::Item>;

  /// create a new `Extend` of the correct type
  fn new_builder(&self) -> Self::Extender;
  /// accumulate the input into an accumulator
  fn extend_into(&self, acc: &mut Self::Extender);
}

#[cfg(feature = "alloc")]
impl ExtendInto for [u8] {
  type Item = u8;
  type Extender = Vec<u8>;

  #[inline]
  fn new_builder(&self) -> Vec<u8> {
    Vec::new()
  }
  #[inline]
  fn extend_into(&self, acc: &mut Vec<u8>) {
    acc.extend(self.iter().cloned());
  }
}

#[cfg(feature = "alloc")]
impl ExtendInto for &[u8] {
  type Item = u8;
  type Extender = Vec<u8>;

  #[inline]
  fn new_builder(&self) -> Vec<u8> {
    Vec::new()
  }
  #[inline]
  fn extend_into(&self, acc: &mut Vec<u8>) {
    acc.extend(self.iter().cloned());
  }
}


#[cfg(feature = "alloc")]
impl ExtendInto for str {
  type Item = char;
  type Extender = String;

  #[inline]
  fn new_builder(&self) -> String {
    String::new()
  }
  #[inline]
  fn extend_into(&self, acc: &mut String) {
    acc.push_str(self);
  }
}

#[cfg(feature = "alloc")]
impl ExtendInto for &str {
  type Item = char;
  type Extender = String;

  #[inline]
  fn new_builder(&self) -> String {
    String::new()
  }
  #[inline]
  fn extend_into(&self, acc: &mut String) {
    acc.push_str(self);
  }
}

#[cfg(feature = "alloc")]
impl ExtendInto for char {
  type Item = char;
  type Extender = String;

  #[inline]
  fn new_builder(&self) -> String {
    String::new()
  }
  #[inline]
  fn extend_into(&self, acc: &mut String) {
    acc.push(*self);
  }
}

/// Helper trait to convert numbers to usize
///
/// by default, usize implements `From<u8>` and `From<u16>` but not
/// `From<u32>` and `From<u64>` because that would be invalid on some
/// platforms. This trait implements the conversion for platforms
/// with 32 and 64 bits pointer platforms
pub trait ToUsize {
  /// converts self to usize
  fn to_usize(&self) -> usize;
}

impl ToUsize for u8 {
  #[inline]
  fn to_usize(&self) -> usize {
    *self as usize
  }
}

impl ToUsize for u16 {
  #[inline]
  fn to_usize(&self) -> usize {
    *self as usize
  }
}

impl ToUsize for usize {
  #[inline]
  fn to_usize(&self) -> usize {
    *self
  }
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl ToUsize for u32 {
  #[inline]
  fn to_usize(&self) -> usize {
    *self as usize
  }
}

#[cfg(target_pointer_width = "64")]
impl ToUsize for u64 {
  #[inline]
  fn to_usize(&self) -> usize {
    *self as usize
  }
}

/// equivalent From implementation to avoid orphan rules in bits parsers
pub trait ErrorConvert<E> {
  /// transform to another error type
  fn convert(self) -> E;
}

impl<I> ErrorConvert<(I, ErrorKind)> for ((I, usize), ErrorKind) {
  fn convert(self) -> (I, ErrorKind) {
    ((self.0).0, self.1)
  }
}

impl<I> ErrorConvert<((I, usize), ErrorKind)> for (I, ErrorKind) {
  fn convert(self) -> ((I, usize), ErrorKind) {
    ((self.0, 0), self.1)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_offset_u8() {
    let s = b"abcd123";
    let a = &s[..];
    let b = &a[2..];
    let c = &a[..4];
    let d = &a[3..5];
    assert_eq!(a.offset(b), 2);
    assert_eq!(a.offset(c), 0);
    assert_eq!(a.offset(d), 3);
  }

  #[test]
  fn test_offset_str() {
    let s = "abcřèÂßÇd123";
    let a = &s[..];
    let b = &a[7..];
    let c = &a[..5];
    let d = &a[5..9];
    assert_eq!(a.offset(b), 7);
    assert_eq!(a.offset(c), 0);
    assert_eq!(a.offset(d), 5);
  }
}
