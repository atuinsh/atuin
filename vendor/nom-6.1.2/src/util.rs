#[cfg(feature = "std")]
use crate::internal::IResult;
#[cfg(feature = "std")]
use std::fmt::Debug;

#[cfg(feature = "std")]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "std")))]
/// Helper trait to show a byte slice as a hex dump
pub trait HexDisplay {
  /// Converts the value of `self` to a hex dump, returning the owned
  /// `String`.
  fn to_hex(&self, chunk_size: usize) -> String;

  /// Converts the value of `self` to a hex dump beginning at `from` address, returning the owned
  /// `String`.
  fn to_hex_from(&self, chunk_size: usize, from: usize) -> String;
}

#[cfg(feature = "std")]
static CHARS: &[u8] = b"0123456789abcdef";

#[cfg(feature = "std")]
impl HexDisplay for [u8] {
  #[allow(unused_variables)]
  fn to_hex(&self, chunk_size: usize) -> String {
    self.to_hex_from(chunk_size, 0)
  }

  #[allow(unused_variables)]
  fn to_hex_from(&self, chunk_size: usize, from: usize) -> String {
    let mut v = Vec::with_capacity(self.len() * 3);
    let mut i = from;
    for chunk in self.chunks(chunk_size) {
      let s = format!("{:08x}", i);
      for &ch in s.as_bytes().iter() {
        v.push(ch);
      }
      v.push(b'\t');

      i += chunk_size;

      for &byte in chunk {
        v.push(CHARS[(byte >> 4) as usize]);
        v.push(CHARS[(byte & 0xf) as usize]);
        v.push(b' ');
      }
      if chunk_size > chunk.len() {
        for j in 0..(chunk_size - chunk.len()) {
          v.push(b' ');
          v.push(b' ');
          v.push(b' ');
        }
      }
      v.push(b'\t');

      for &byte in chunk {
        if (byte >= 32 && byte <= 126) || byte >= 128 {
          v.push(byte);
        } else {
          v.push(b'.');
        }
      }
      v.push(b'\n');
    }

    String::from_utf8_lossy(&v[..]).into_owned()
  }
}

#[cfg(feature = "std")]
impl HexDisplay for str {
  #[allow(unused_variables)]
  fn to_hex(&self, chunk_size: usize) -> String {
    self.to_hex_from(chunk_size, 0)
  }

  #[allow(unused_variables)]
  fn to_hex_from(&self, chunk_size: usize, from: usize) -> String {
    self.as_bytes().to_hex_from(chunk_size, from)
  }
}

#[doc(hidden)]
#[macro_export]
macro_rules! nom_line (
  () => (line!());
);

#[doc(hidden)]
#[macro_export]
macro_rules! nom_println (
  ($($args:tt)*) => (println!($($args)*));
);

#[doc(hidden)]
#[macro_export]
macro_rules! nom_stringify (
  ($($args:tt)*) => (stringify!($($args)*));
);

/// Prints a message if the parser fails.
///
/// The message prints the `Error` or `Incomplete`
/// and the parser's calling code
///
/// ```
/// # #[macro_use] extern crate nom;
/// # fn main() {
///    named!(f, dbg_basic!( tag!( "abcd" ) ) );
///
///    let a = &b"efgh"[..];
///
///    // Will print the following message:
///    // Error(Position(0, [101, 102, 103, 104])) at l.5 by ' tag ! ( "abcd" ) '
///    f(a);
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! dbg_basic (
  ($i: expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::lib::std::result::Result::*;
      let l = nom_line!();
      match $submac!($i, $($args)*) {
        Err(e) => {
          nom_println!("Err({:?}) at l.{} by ' {} '", e, l, nom_stringify!($submac!($($args)*)));
          Err(e)
        },
        a => a,
      }
    }
  );

  ($i:expr, $f:ident) => (
      dbg_basic!($i, call!($f));
  );
);

/// Prints a message and the input if the parser fails.
///
/// The message prints the `Error` or `Incomplete`
/// and the parser's calling code.
///
/// It also displays the input in hexdump format
///
/// ```rust
/// use nom::{IResult, dbg_dmp, bytes::complete::tag};
///
/// fn f(i: &[u8]) -> IResult<&[u8], &[u8]> {
///   dbg_dmp(tag("abcd"), "tag")(i)
/// }
///
///   let a = &b"efghijkl"[..];
///
/// // Will print the following message:
/// // Error(Position(0, [101, 102, 103, 104, 105, 106, 107, 108])) at l.5 by ' tag ! ( "abcd" ) '
/// // 00000000        65 66 67 68 69 6a 6b 6c         efghijkl
/// f(a);
/// ```
#[cfg(feature = "std")]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "std")))]
pub fn dbg_dmp<'a, F, O, E: Debug>(
  f: F,
  context: &'static str,
) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], O, E>
where
  F: Fn(&'a [u8]) -> IResult<&'a [u8], O, E>,
{
  move |i: &'a [u8]| match f(i) {
    Err(e) => {
      println!("{}: Error({:?}) at:\n{}", context, e, i.to_hex(8));
      Err(e)
    }
    a => a,
  }
}

/// Prints a message and the input if the parser fails.
///
/// The message prints the `Error` or `Incomplete`
/// and the parser's calling code.
///
/// It also displays the input in hexdump format
///
/// ```ignore
/// # #[macro_use] extern crate nom;
/// # fn main() {
///    named!(f, dbg_dmp!( tag!( "abcd" ) ) );
///
///    let a = &b"efghijkl"[..];
///
///    // Will print the following message:
///    // Error(Position(0, [101, 102, 103, 104, 105, 106, 107, 108])) at l.5 by ' tag ! ( "abcd" ) '
///    // 00000000        65 66 67 68 69 6a 6b 6c         efghijkl
///    f(a);
/// # }
#[macro_export(local_inner_macros)]
#[cfg(feature = "std")]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "std")))]
macro_rules! dbg_dmp (
  ($i: expr, $submac:ident!( $($args:tt)* )) => (
    {
      use $crate::HexDisplay;
      let l = nom_line!();
      match $submac!($i, $($args)*) {
        Err(e) => {
          nom_println!("Error({:?}) at l.{} by ' {} '\n{}", e, l, nom_stringify!($submac!($($args)*)), $i.to_hex(8));
          Err(e)
        },
        a => a,
      }
    }
  );

  ($i:expr, $f:ident) => (
      dbg_dmp!($i, call!($f));
  );
);
