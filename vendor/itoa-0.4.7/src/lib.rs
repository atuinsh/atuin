//! [![github]](https://github.com/dtolnay/itoa)&ensp;[![crates-io]](https://crates.io/crates/itoa)&ensp;[![docs-rs]](https://docs.rs/itoa)
//!
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//! [crates-io]: https://img.shields.io/badge/crates.io-fc8d62?style=for-the-badge&labelColor=555555&logo=rust
//! [docs-rs]: https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K
//!
//! <br>
//!
//! This crate provides fast functions for printing integer primitives to an
//! [`io::Write`] or a [`fmt::Write`]. The implementation comes straight from
//! [libcore] but avoids the performance penalty of going through
//! [`fmt::Formatter`].
//!
//! See also [`dtoa`] for printing floating point primitives.
//!
//! [`io::Write`]: https://doc.rust-lang.org/std/io/trait.Write.html
//! [`fmt::Write`]: https://doc.rust-lang.org/core/fmt/trait.Write.html
//! [libcore]: https://github.com/rust-lang/rust/blob/b8214dc6c6fc20d0a660fb5700dca9ebf51ebe89/src/libcore/fmt/num.rs#L201-L254
//! [`fmt::Formatter`]: https://doc.rust-lang.org/std/fmt/struct.Formatter.html
//! [`dtoa`]: https://github.com/dtolnay/dtoa
//!
//! <br>
//!
//! # Performance (lower is better)
//!
//! ![performance](https://raw.githubusercontent.com/dtolnay/itoa/master/performance.png)
//!
//! <br>
//!
//! # Examples
//!
//! ```edition2018
//! use std::{fmt, io};
//!
//! fn demo_itoa_write() -> io::Result<()> {
//!     // Write to a vector or other io::Write.
//!     let mut buf = Vec::new();
//!     itoa::write(&mut buf, 128u64)?;
//!     println!("{:?}", buf);
//!
//!     // Write to a stack buffer.
//!     let mut bytes = [0u8; 20];
//!     let n = itoa::write(&mut bytes[..], 128u64)?;
//!     println!("{:?}", &bytes[..n]);
//!
//!     Ok(())
//! }
//!
//! fn demo_itoa_fmt() -> fmt::Result {
//!     // Write to a string.
//!     let mut s = String::new();
//!     itoa::fmt(&mut s, 128u64)?;
//!     println!("{}", s);
//!
//!     Ok(())
//! }
//! ```

#![doc(html_root_url = "https://docs.rs/itoa/0.4.7")]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "cargo-clippy", allow(renamed_and_removed_lints))]
#![cfg_attr(
    feature = "cargo-clippy",
    allow(const_static_lifetime, transmute_ptr_to_ptr)
)]

#[cfg(feature = "i128")]
mod udiv128;

#[cfg(feature = "std")]
use std::{fmt, io, mem, ptr, slice, str};

#[cfg(not(feature = "std"))]
use core::{fmt, mem, ptr, slice, str};

/// Write integer to an `io::Write`.
#[cfg(feature = "std")]
#[inline]
pub fn write<W: io::Write, V: Integer>(mut wr: W, value: V) -> io::Result<usize> {
    let mut buf = Buffer::new();
    let s = buf.format(value);
    match wr.write_all(s.as_bytes()) {
        Ok(()) => Ok(s.len()),
        Err(e) => Err(e),
    }
}

/// Write integer to an `fmt::Write`.
#[inline]
pub fn fmt<W: fmt::Write, V: Integer>(mut wr: W, value: V) -> fmt::Result {
    let mut buf = Buffer::new();
    wr.write_str(buf.format(value))
}

/// A safe API for formatting integers to text.
///
/// # Example
///
/// ```
/// let mut buffer = itoa::Buffer::new();
/// let printed = buffer.format(1234);
/// assert_eq!(printed, "1234");
/// ```
#[derive(Copy)]
pub struct Buffer {
    bytes: [u8; I128_MAX_LEN],
}

impl Default for Buffer {
    #[inline]
    fn default() -> Buffer {
        Buffer::new()
    }
}

impl Clone for Buffer {
    #[inline]
    fn clone(&self) -> Self {
        Buffer::new()
    }
}

impl Buffer {
    /// This is a cheap operation; you don't need to worry about reusing buffers
    /// for efficiency.
    #[inline]
    #[allow(deprecated)]
    pub fn new() -> Buffer {
        Buffer {
            bytes: unsafe { mem::uninitialized() },
        }
    }

    /// Print an integer into this buffer and return a reference to its string representation
    /// within the buffer.
    pub fn format<I: Integer>(&mut self, i: I) -> &str {
        i.write(self)
    }
}

// Seal to prevent downstream implementations of the Integer trait.
mod private {
    pub trait Sealed {}
}

/// An integer that can be formatted by `itoa::write` and `itoa::fmt`.
///
/// This trait is sealed and cannot be implemented for types outside of itoa.
pub trait Integer: private::Sealed {
    // Not public API.
    #[doc(hidden)]
    fn write(self, buf: &mut Buffer) -> &str;
}

trait IntegerPrivate<B> {
    fn write_to(self, buf: &mut B) -> &[u8];
}

const DEC_DIGITS_LUT: &'static [u8] = b"\
      0001020304050607080910111213141516171819\
      2021222324252627282930313233343536373839\
      4041424344454647484950515253545556575859\
      6061626364656667686970717273747576777879\
      8081828384858687888990919293949596979899";

// Adaptation of the original implementation at
// https://github.com/rust-lang/rust/blob/b8214dc6c6fc20d0a660fb5700dca9ebf51ebe89/src/libcore/fmt/num.rs#L188-L266
macro_rules! impl_IntegerCommon {
    ($max_len:expr, $t:ident) => {
        impl Integer for $t {
            #[inline]
            fn write(self, buf: &mut Buffer) -> &str {
                unsafe {
                    debug_assert!($max_len <= I128_MAX_LEN);
                    let buf = mem::transmute::<&mut [u8; I128_MAX_LEN], &mut [u8; $max_len]>(
                        &mut buf.bytes,
                    );
                    let bytes = self.write_to(buf);
                    str::from_utf8_unchecked(bytes)
                }
            }
        }

        impl private::Sealed for $t {}
    };
}

macro_rules! impl_Integer {
    ($($max_len:expr => $t:ident),* as $conv_fn:ident) => {$(
        impl_IntegerCommon!($max_len, $t);

        impl IntegerPrivate<[u8; $max_len]> for $t {
            #[allow(unused_comparisons)]
            #[inline]
            fn write_to(self, buf: &mut [u8; $max_len]) -> &[u8] {
                let is_nonnegative = self >= 0;
                let mut n = if is_nonnegative {
                    self as $conv_fn
                } else {
                    // convert the negative num to positive by summing 1 to it's 2 complement
                    (!(self as $conv_fn)).wrapping_add(1)
                };
                let mut curr = buf.len() as isize;
                let buf_ptr = buf.as_mut_ptr();
                let lut_ptr = DEC_DIGITS_LUT.as_ptr();

                unsafe {
                    // need at least 16 bits for the 4-characters-at-a-time to work.
                    if mem::size_of::<$t>() >= 2 {
                        // eagerly decode 4 characters at a time
                        while n >= 10000 {
                            let rem = (n % 10000) as isize;
                            n /= 10000;

                            let d1 = (rem / 100) << 1;
                            let d2 = (rem % 100) << 1;
                            curr -= 4;
                            ptr::copy_nonoverlapping(lut_ptr.offset(d1), buf_ptr.offset(curr), 2);
                            ptr::copy_nonoverlapping(lut_ptr.offset(d2), buf_ptr.offset(curr + 2), 2);
                        }
                    }

                    // if we reach here numbers are <= 9999, so at most 4 chars long
                    let mut n = n as isize; // possibly reduce 64bit math

                    // decode 2 more chars, if > 2 chars
                    if n >= 100 {
                        let d1 = (n % 100) << 1;
                        n /= 100;
                        curr -= 2;
                        ptr::copy_nonoverlapping(lut_ptr.offset(d1), buf_ptr.offset(curr), 2);
                    }

                    // decode last 1 or 2 chars
                    if n < 10 {
                        curr -= 1;
                        *buf_ptr.offset(curr) = (n as u8) + b'0';
                    } else {
                        let d1 = n << 1;
                        curr -= 2;
                        ptr::copy_nonoverlapping(lut_ptr.offset(d1), buf_ptr.offset(curr), 2);
                    }

                    if !is_nonnegative {
                        curr -= 1;
                        *buf_ptr.offset(curr) = b'-';
                    }
                }

                let len = buf.len() - curr as usize;
                unsafe { slice::from_raw_parts(buf_ptr.offset(curr), len) }
            }
        }
    )*};
}

const I8_MAX_LEN: usize = 4;
const U8_MAX_LEN: usize = 3;
const I16_MAX_LEN: usize = 6;
const U16_MAX_LEN: usize = 5;
const I32_MAX_LEN: usize = 11;
const U32_MAX_LEN: usize = 10;
const I64_MAX_LEN: usize = 20;
const U64_MAX_LEN: usize = 20;

impl_Integer!(
    I8_MAX_LEN => i8,
    U8_MAX_LEN => u8,
    I16_MAX_LEN => i16,
    U16_MAX_LEN => u16,
    I32_MAX_LEN => i32,
    U32_MAX_LEN => u32
    as u32);

impl_Integer!(I64_MAX_LEN => i64, U64_MAX_LEN => u64 as u64);

#[cfg(target_pointer_width = "16")]
impl_Integer!(I16_MAX_LEN => isize, U16_MAX_LEN => usize as u16);

#[cfg(target_pointer_width = "32")]
impl_Integer!(I32_MAX_LEN => isize, U32_MAX_LEN => usize as u32);

#[cfg(target_pointer_width = "64")]
impl_Integer!(I64_MAX_LEN => isize, U64_MAX_LEN => usize as u64);

#[cfg(all(feature = "i128"))]
macro_rules! impl_Integer128 {
    ($($max_len:expr => $t:ident),*) => {$(
        impl_IntegerCommon!($max_len, $t);

        impl IntegerPrivate<[u8; $max_len]> for $t {
            #[allow(unused_comparisons)]
            #[inline]
            fn write_to(self, buf: &mut [u8; $max_len]) -> &[u8] {
                let is_nonnegative = self >= 0;
                let n = if is_nonnegative {
                    self as u128
                } else {
                    // convert the negative num to positive by summing 1 to it's 2 complement
                    (!(self as u128)).wrapping_add(1)
                };
                let mut curr = buf.len() as isize;
                let buf_ptr = buf.as_mut_ptr();

                unsafe {
                    // Divide by 10^19 which is the highest power less than 2^64.
                    let (n, rem) = udiv128::udivmod_1e19(n);
                    let buf1 = buf_ptr.offset(curr - U64_MAX_LEN as isize) as *mut [u8; U64_MAX_LEN];
                    curr -= rem.write_to(&mut *buf1).len() as isize;

                    if n != 0 {
                        // Memset the base10 leading zeros of rem.
                        let target = buf.len() as isize - 19;
                        ptr::write_bytes(buf_ptr.offset(target), b'0', (curr - target) as usize);
                        curr = target;

                        // Divide by 10^19 again.
                        let (n, rem) = udiv128::udivmod_1e19(n);
                        let buf2 = buf_ptr.offset(curr - U64_MAX_LEN as isize) as *mut [u8; U64_MAX_LEN];
                        curr -= rem.write_to(&mut *buf2).len() as isize;

                        if n != 0 {
                            // Memset the leading zeros.
                            let target = buf.len() as isize - 38;
                            ptr::write_bytes(buf_ptr.offset(target), b'0', (curr - target) as usize);
                            curr = target;

                            // There is at most one digit left
                            // because u128::max / 10^19 / 10^19 is 3.
                            curr -= 1;
                            *buf_ptr.offset(curr) = (n as u8) + b'0';
                        }
                    }

                    if !is_nonnegative {
                        curr -= 1;
                        *buf_ptr.offset(curr) = b'-';
                    }

                    let len = buf.len() - curr as usize;
                    slice::from_raw_parts(buf_ptr.offset(curr), len)
                }
            }
        }
    )*};
}

#[cfg(all(feature = "i128"))]
const U128_MAX_LEN: usize = 39;
const I128_MAX_LEN: usize = 40;

#[cfg(all(feature = "i128"))]
impl_Integer128!(I128_MAX_LEN => i128, U128_MAX_LEN => u128);
