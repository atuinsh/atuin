// Copyright 2018 Developers of the Rand project.
// Copyright 2013 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The exponential distribution.
#![allow(deprecated)]

use crate::distributions::utils::ziggurat;
use crate::distributions::{ziggurat_tables, Distribution};
use crate::Rng;

/// Samples floating-point numbers according to the exponential distribution,
/// with rate parameter `λ = 1`. This is equivalent to `Exp::new(1.0)` or
/// sampling with `-rng.gen::<f64>().ln()`, but faster.
///
/// See `Exp` for the general exponential distribution.
///
/// Implemented via the ZIGNOR variant[^1] of the Ziggurat method. The exact
/// description in the paper was adjusted to use tables for the exponential
/// distribution rather than normal.
///
/// [^1]: Jurgen A. Doornik (2005). [*An Improved Ziggurat Method to
///       Generate Normal Random Samples*](
///       https://www.doornik.com/research/ziggurat.pdf).
///       Nuffield College, Oxford
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Exp1;

// This could be done via `-rng.gen::<f64>().ln()` but that is slower.
impl Distribution<f64> for Exp1 {
    #[inline]
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        #[inline]
        fn pdf(x: f64) -> f64 {
            (-x).exp()
        }
        #[inline]
        fn zero_case<R: Rng + ?Sized>(rng: &mut R, _u: f64) -> f64 {
            ziggurat_tables::ZIG_EXP_R - rng.gen::<f64>().ln()
        }

        ziggurat(
            rng,
            false,
            &ziggurat_tables::ZIG_EXP_X,
            &ziggurat_tables::ZIG_EXP_F,
            pdf,
            zero_case,
        )
    }
}

/// The exponential distribution `Exp(lambda)`.
///
/// This distribution has density function: `f(x) = lambda * exp(-lambda * x)`
/// for `x > 0`.
///
/// Note that [`Exp1`](crate::distributions::Exp1) is an optimised implementation for `lambda = 1`.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Exp {
    /// `lambda` stored as `1/lambda`, since this is what we scale by.
    lambda_inverse: f64,
}

impl Exp {
    /// Construct a new `Exp` with the given shape parameter
    /// `lambda`. Panics if `lambda <= 0`.
    #[inline]
    pub fn new(lambda: f64) -> Exp {
        assert!(lambda > 0.0, "Exp::new called with `lambda` <= 0");
        Exp {
            lambda_inverse: 1.0 / lambda,
        }
    }
}

impl Distribution<f64> for Exp {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let n: f64 = rng.sample(Exp1);
        n * self.lambda_inverse
    }
}

#[cfg(test)]
mod test {
    use super::Exp;
    use crate::distributions::Distribution;

    #[test]
    fn test_exp() {
        let exp = Exp::new(10.0);
        let mut rng = crate::test::rng(221);
        for _ in 0..1000 {
            assert!(exp.sample(&mut rng) >= 0.0);
        }
    }
    #[test]
    #[should_panic]
    fn test_exp_invalid_lambda_zero() {
        Exp::new(0.0);
    }
    #[test]
    #[should_panic]
    fn test_exp_invalid_lambda_neg() {
        Exp::new(-10.0);
    }
}
