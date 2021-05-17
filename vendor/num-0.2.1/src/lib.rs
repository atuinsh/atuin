// Copyright 2014-2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A collection of numeric types and traits for Rust.
//!
//! This includes new types for big integers, rationals, and complex numbers,
//! new traits for generic programming on numeric properties like `Integer`,
//! and generic range iterators.
//!
//! ## Example
//!
//! This example uses the BigRational type and [Newton's method][newt] to
//! approximate a square root to arbitrary precision:
//!
//! ```
//! extern crate num;
//! # #[cfg(feature = "std")]
//! # mod test {
//!
//! use num::FromPrimitive;
//! use num::bigint::BigInt;
//! use num::rational::{Ratio, BigRational};
//!
//! # pub
//! fn approx_sqrt(number: u64, iterations: usize) -> BigRational {
//!     let start: Ratio<BigInt> = Ratio::from_integer(FromPrimitive::from_u64(number).unwrap());
//!     let mut approx = start.clone();
//!
//!     for _ in 0..iterations {
//!         approx = (&approx + (&start / &approx)) /
//!             Ratio::from_integer(FromPrimitive::from_u64(2).unwrap());
//!     }
//!
//!     approx
//! }
//! # }
//! # #[cfg(not(feature = "std"))]
//! # mod test { pub fn approx_sqrt(n: u64, _: usize) -> u64 { n } }
//! # use test::approx_sqrt;
//!
//! fn main() {
//!     println!("{}", approx_sqrt(10, 4)); // prints 4057691201/1283082416
//! }
//!
//! ```
//!
//! [newt]: https://en.wikipedia.org/wiki/Methods_of_computing_square_roots#Babylonian_method
//!
//! ## Compatibility
//!
//! The `num` crate is tested for rustc 1.15 and greater.

#![doc(html_root_url = "https://docs.rs/num/0.2")]
#![no_std]

#[cfg(feature = "std")]
extern crate num_bigint;
extern crate num_complex;
extern crate num_integer;
extern crate num_iter;
extern crate num_rational;
extern crate num_traits;

#[cfg(feature = "std")]
pub use num_bigint::{BigInt, BigUint};

pub use num_complex::Complex;

#[cfg(feature = "std")]
pub use num_rational::BigRational;
pub use num_rational::Rational;

pub use num_integer::Integer;

pub use num_iter::{range, range_inclusive, range_step, range_step_inclusive};

#[cfg(feature = "std")]
pub use num_traits::Float;
pub use num_traits::{
    abs, abs_sub, cast, checked_pow, clamp, one, pow, signum, zero, Bounded, CheckedAdd,
    CheckedDiv, CheckedMul, CheckedSub, FromPrimitive, Num, NumCast, One, PrimInt, Saturating,
    Signed, ToPrimitive, Unsigned, Zero,
};

#[cfg(feature = "std")]
pub mod bigint {
    pub use num_bigint::*;
}

pub mod complex {
    pub use num_complex::*;
}

pub mod integer {
    pub use num_integer::*;
}

pub mod iter {
    pub use num_iter::*;
}

pub mod traits {
    pub use num_traits::*;
}

pub mod rational {
    pub use num_rational::*;
}
