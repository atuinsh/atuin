// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A Big integer (signed version: `BigInt`, unsigned version: `BigUint`).
//!
//! A `BigUint` is represented as a vector of `BigDigit`s.
//! A `BigInt` is a combination of `BigUint` and `Sign`.
//!
//! Common numerical operations are overloaded, so we can treat them
//! the same way we treat other numbers.
//!
//! ## Example
//!
//! ```rust
//! extern crate num_bigint;
//! extern crate num_traits;
//!
//! # fn main() {
//! use num_bigint::BigUint;
//! use num_traits::{Zero, One};
//! use std::mem::replace;
//!
//! // Calculate large fibonacci numbers.
//! fn fib(n: usize) -> BigUint {
//!     let mut f0: BigUint = Zero::zero();
//!     let mut f1: BigUint = One::one();
//!     for _ in 0..n {
//!         let f2 = f0 + &f1;
//!         // This is a low cost way of swapping f0 with f1 and f1 with f2.
//!         f0 = replace(&mut f1, f2);
//!     }
//!     f0
//! }
//!
//! // This is a very large number.
//! println!("fib(1000) = {}", fib(1000));
//! # }
//! ```
//!
//! It's easy to generate large random numbers:
//!
//! ```rust
//! # #[cfg(feature = "rand")]
//! extern crate rand;
//! extern crate num_bigint as bigint;
//!
//! # #[cfg(feature = "rand")]
//! # fn main() {
//! use bigint::{ToBigInt, RandBigInt};
//!
//! let mut rng = rand::thread_rng();
//! let a = rng.gen_bigint(1000);
//!
//! let low = -10000.to_bigint().unwrap();
//! let high = 10000.to_bigint().unwrap();
//! let b = rng.gen_bigint_range(&low, &high);
//!
//! // Probably an even larger number.
//! println!("{}", a * b);
//! # }
//!
//! # #[cfg(not(feature = "rand"))]
//! # fn main() {
//! # }
//! ```
//!
//! See the "Features" section for instructions for enabling random number generation.
//!
//! ## Features
//!
//! The `std` crate feature is mandatory and enabled by default.  If you depend on
//! `num-bigint` with `default-features = false`, you must manually enable the
//! `std` feature yourself.  In the future, we hope to support `#![no_std]` with
//! the `alloc` crate when `std` is not enabled.
//!
//! Implementations for `i128` and `u128` are only available with Rust 1.26 and
//! later.  The build script automatically detects this, but you can make it
//! mandatory by enabling the `i128` crate feature.
//!
//! ### Random Generation
//!
//! `num-bigint` supports the generation of random big integers when the `rand`
//! feature is enabled. To enable it include rand as
//!
//! ```toml
//! rand = "0.5"
//! num-bigint = { version = "0.2", features = ["rand"] }
//! ```
//!
//! Note that you must use the version of `rand` that `num-bigint` is compatible
//! with: `0.5`.
//!
//!
//! ## Compatibility
//!
//! The `num-bigint` crate is tested for rustc 1.15 and greater.

#![doc(html_root_url = "https://docs.rs/num-bigint/0.2")]
// We don't actually support `no_std` yet, and probably won't until `alloc` is stable.  We're just
// reserving this ability with the "std" feature now, and compilation will fail without.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "rand")]
extern crate rand;
#[cfg(feature = "serde")]
extern crate serde;

extern crate num_integer as integer;
extern crate num_traits as traits;
#[cfg(feature = "quickcheck")]
extern crate quickcheck;

use std::error::Error;
use std::fmt;

#[macro_use]
mod macros;

mod bigint;
mod biguint;

#[cfg(feature = "rand")]
mod bigrand;

#[cfg(target_pointer_width = "32")]
type UsizePromotion = u32;
#[cfg(target_pointer_width = "64")]
type UsizePromotion = u64;

#[cfg(target_pointer_width = "32")]
type IsizePromotion = i32;
#[cfg(target_pointer_width = "64")]
type IsizePromotion = i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseBigIntError {
    kind: BigIntErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BigIntErrorKind {
    Empty,
    InvalidDigit,
}

impl ParseBigIntError {
    fn __description(&self) -> &str {
        use BigIntErrorKind::*;
        match self.kind {
            Empty => "cannot parse integer from empty string",
            InvalidDigit => "invalid digit found in string",
        }
    }

    fn empty() -> Self {
        ParseBigIntError {
            kind: BigIntErrorKind::Empty,
        }
    }

    fn invalid() -> Self {
        ParseBigIntError {
            kind: BigIntErrorKind::InvalidDigit,
        }
    }
}

impl fmt::Display for ParseBigIntError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.__description().fmt(f)
    }
}

impl Error for ParseBigIntError {
    fn description(&self) -> &str {
        self.__description()
    }
}

pub use biguint::BigUint;
pub use biguint::ToBigUint;

pub use bigint::BigInt;
pub use bigint::Sign;
pub use bigint::ToBigInt;

#[cfg(feature = "rand")]
pub use bigrand::{RandBigInt, RandomBits, UniformBigInt, UniformBigUint};

mod big_digit {
    /// A `BigDigit` is a `BigUint`'s composing element.
    pub type BigDigit = u32;

    /// A `DoubleBigDigit` is the internal type used to do the computations.  Its
    /// size is the double of the size of `BigDigit`.
    pub type DoubleBigDigit = u64;

    /// A `SignedDoubleBigDigit` is the signed version of `DoubleBigDigit`.
    pub type SignedDoubleBigDigit = i64;

    // `DoubleBigDigit` size dependent
    pub const BITS: usize = 32;

    const LO_MASK: DoubleBigDigit = (-1i32 as DoubleBigDigit) >> BITS;

    #[inline]
    fn get_hi(n: DoubleBigDigit) -> BigDigit {
        (n >> BITS) as BigDigit
    }
    #[inline]
    fn get_lo(n: DoubleBigDigit) -> BigDigit {
        (n & LO_MASK) as BigDigit
    }

    /// Split one `DoubleBigDigit` into two `BigDigit`s.
    #[inline]
    pub fn from_doublebigdigit(n: DoubleBigDigit) -> (BigDigit, BigDigit) {
        (get_hi(n), get_lo(n))
    }

    /// Join two `BigDigit`s into one `DoubleBigDigit`
    #[inline]
    pub fn to_doublebigdigit(hi: BigDigit, lo: BigDigit) -> DoubleBigDigit {
        DoubleBigDigit::from(lo) | (DoubleBigDigit::from(hi) << BITS)
    }
}
