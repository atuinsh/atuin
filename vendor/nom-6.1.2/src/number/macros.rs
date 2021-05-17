//! Parsers recognizing numbers

/// If the parameter is `nom::Endianness::Big`, parse a big endian u16 integer,
/// otherwise a little endian u16 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<u16>, u16!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01abcd"), Ok((&b"abcd"[..], 0x0001)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(1))));
///
/// named!(le<u16>, u16!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01abcd"), Ok((&b"abcd"[..], 0x0100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(1))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! u16 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_u16($i) } else { $crate::number::streaming::le_u16($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian u32 integer,
/// otherwise a little endian u32 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<u32>, u32!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03abcd"), Ok((&b"abcd"[..], 0x00010203)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(3))));
///
/// named!(le<u32>, u32!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03abcd"), Ok((&b"abcd"[..], 0x03020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(3))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! u32 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_u32($i) } else { $crate::number::streaming::le_u32($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian u64 integer,
/// otherwise a little endian u64 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<u64>, u64!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03\x04\x05\x06\x07abcd"), Ok((&b"abcd"[..], 0x0001020304050607)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(7))));
///
/// named!(le<u64>, u64!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03\x04\x05\x06\x07abcd"), Ok((&b"abcd"[..], 0x0706050403020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(7))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! u64 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_u64($i) } else { $crate::number::streaming::le_u64($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian u128 integer,
/// otherwise a little endian u128 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<u128>, u128!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x10\x11\x12\x13\x14\x15abcd"), Ok((&b"abcd"[..], 0x00010203040506070809101112131415)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(15))));
///
/// named!(le<u128>, u128!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x10\x11\x12\x13\x14\x15abcd"), Ok((&b"abcd"[..], 0x15141312111009080706050403020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(15))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
#[cfg(stable_i128)]
macro_rules! u128 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_u128($i) } else { $crate::number::streaming::le_u128($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian i16 integer,
/// otherwise a little endian i16 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<i16>, i16!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01abcd"), Ok((&b"abcd"[..], 0x0001)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(1))));
///
/// named!(le<i16>, i16!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01abcd"), Ok((&b"abcd"[..], 0x0100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(1))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! i16 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_i16($i) } else { $crate::number::streaming::le_i16($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian i32 integer,
/// otherwise a little endian i32 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<i32>, i32!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03abcd"), Ok((&b"abcd"[..], 0x00010203)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(3))));
///
/// named!(le<i32>, i32!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03abcd"), Ok((&b"abcd"[..], 0x03020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(3))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! i32 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_i32($i) } else { $crate::number::streaming::le_i32($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian i64 integer,
/// otherwise a little endian i64 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<i64>, i64!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03\x04\x05\x06\x07abcd"), Ok((&b"abcd"[..], 0x0001020304050607)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(7))));
///
/// named!(le<i64>, i64!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03\x04\x05\x06\x07abcd"), Ok((&b"abcd"[..], 0x0706050403020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(7))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! i64 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_i64($i) } else { $crate::number::streaming::le_i64($i) } } ););

/// If the parameter is `nom::Endianness::Big`, parse a big endian i64 integer,
/// otherwise a little endian i64 integer.
///
/// ```rust
/// # #[macro_use] extern crate nom;
/// # use nom::{Err, Needed};
/// use nom::number::Endianness;
///
/// # fn main() {
/// named!(be<i128>, i128!(Endianness::Big));
///
/// assert_eq!(be(b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x10\x11\x12\x13\x14\x15abcd"), Ok((&b"abcd"[..], 0x00010203040506070809101112131415)));
/// assert_eq!(be(b"\x01"), Err(Err::Incomplete(Needed::new(15))));
///
/// named!(le<i128>, i128!(Endianness::Little));
///
/// assert_eq!(le(b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x10\x11\x12\x13\x14\x15abcd"), Ok((&b"abcd"[..], 0x15141312111009080706050403020100)));
/// assert_eq!(le(b"\x01"), Err(Err::Incomplete(Needed::new(15))));
/// # }
/// ```
#[macro_export(local_inner_macros)]
#[cfg(stable_i128)]
macro_rules! i128 ( ($i:expr, $e:expr) => ( {if $crate::number::Endianness::Big == $e { $crate::number::streaming::be_i128($i) } else { $crate::number::streaming::le_i128($i) } } ););

#[cfg(test)]
mod tests {
  use crate::number::Endianness;

  #[test]
  fn configurable_endianness() {
    named!(be_tst16<u16>, u16!(Endianness::Big));
    named!(le_tst16<u16>, u16!(Endianness::Little));
    assert_eq!(be_tst16(&[0x80, 0x00]), Ok((&b""[..], 32_768_u16)));
    assert_eq!(le_tst16(&[0x80, 0x00]), Ok((&b""[..], 128_u16)));

    named!(be_tst32<u32>, u32!(Endianness::Big));
    named!(le_tst32<u32>, u32!(Endianness::Little));
    assert_eq!(
      be_tst32(&[0x12, 0x00, 0x60, 0x00]),
      Ok((&b""[..], 302_014_464_u32))
    );
    assert_eq!(
      le_tst32(&[0x12, 0x00, 0x60, 0x00]),
      Ok((&b""[..], 6_291_474_u32))
    );

    named!(be_tst64<u64>, u64!(Endianness::Big));
    named!(le_tst64<u64>, u64!(Endianness::Little));
    assert_eq!(
      be_tst64(&[0x12, 0x00, 0x60, 0x00, 0x12, 0x00, 0x80, 0x00]),
      Ok((&b""[..], 1_297_142_246_100_992_000_u64))
    );
    assert_eq!(
      le_tst64(&[0x12, 0x00, 0x60, 0x00, 0x12, 0x00, 0x80, 0x00]),
      Ok((&b""[..], 36_028_874_334_666_770_u64))
    );

    named!(be_tsti16<i16>, i16!(Endianness::Big));
    named!(le_tsti16<i16>, i16!(Endianness::Little));
    assert_eq!(be_tsti16(&[0x00, 0x80]), Ok((&b""[..], 128_i16)));
    assert_eq!(le_tsti16(&[0x00, 0x80]), Ok((&b""[..], -32_768_i16)));

    named!(be_tsti32<i32>, i32!(Endianness::Big));
    named!(le_tsti32<i32>, i32!(Endianness::Little));
    assert_eq!(
      be_tsti32(&[0x00, 0x12, 0x60, 0x00]),
      Ok((&b""[..], 1_204_224_i32))
    );
    assert_eq!(
      le_tsti32(&[0x00, 0x12, 0x60, 0x00]),
      Ok((&b""[..], 6_296_064_i32))
    );

    named!(be_tsti64<i64>, i64!(Endianness::Big));
    named!(le_tsti64<i64>, i64!(Endianness::Little));
    assert_eq!(
      be_tsti64(&[0x00, 0xFF, 0x60, 0x00, 0x12, 0x00, 0x80, 0x00]),
      Ok((&b""[..], 71_881_672_479_506_432_i64))
    );
    assert_eq!(
      le_tsti64(&[0x00, 0xFF, 0x60, 0x00, 0x12, 0x00, 0x80, 0x00]),
      Ok((&b""[..], 36_028_874_334_732_032_i64))
    );
  }

  //FIXME
  /*
  #[test]
  #[cfg(feature = "std")]
  fn manual_configurable_endianness_test() {
    let x = 1;
    let int_parse: Box<Fn(&[u8]) -> IResult<&[u8], u16, (&[u8], ErrorKind)>> = if x == 2 {
      Box::new(be_u16)
    } else {
      Box::new(le_u16)
    };
    println!("{:?}", int_parse(&b"3"[..]));
    assert_eq!(int_parse(&[0x80, 0x00]), Ok((&b""[..], 128_u16)));
  }
  */
}
