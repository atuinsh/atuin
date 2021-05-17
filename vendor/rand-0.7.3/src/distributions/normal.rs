// Copyright 2018 Developers of the Rand project.
// Copyright 2013 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The normal and derived distributions.
#![allow(deprecated)]

use crate::distributions::utils::ziggurat;
use crate::distributions::{ziggurat_tables, Distribution, Open01};
use crate::Rng;

/// Samples floating-point numbers according to the normal distribution
/// `N(0, 1)` (a.k.a. a standard normal, or Gaussian). This is equivalent to
/// `Normal::new(0.0, 1.0)` but faster.
///
/// See `Normal` for the general normal distribution.
///
/// Implemented via the ZIGNOR variant[^1] of the Ziggurat method.
///
/// [^1]: Jurgen A. Doornik (2005). [*An Improved Ziggurat Method to
///       Generate Normal Random Samples*](
///       https://www.doornik.com/research/ziggurat.pdf).
///       Nuffield College, Oxford
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct StandardNormal;

impl Distribution<f64> for StandardNormal {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        #[inline]
        fn pdf(x: f64) -> f64 {
            (-x * x / 2.0).exp()
        }
        #[inline]
        fn zero_case<R: Rng + ?Sized>(rng: &mut R, u: f64) -> f64 {
            // compute a random number in the tail by hand

            // strange initial conditions, because the loop is not
            // do-while, so the condition should be true on the first
            // run, they get overwritten anyway (0 < 1, so these are
            // good).
            let mut x = 1.0f64;
            let mut y = 0.0f64;

            while -2.0 * y < x * x {
                let x_: f64 = rng.sample(Open01);
                let y_: f64 = rng.sample(Open01);

                x = x_.ln() / ziggurat_tables::ZIG_NORM_R;
                y = y_.ln();
            }

            if u < 0.0 {
                x - ziggurat_tables::ZIG_NORM_R
            } else {
                ziggurat_tables::ZIG_NORM_R - x
            }
        }

        ziggurat(
            rng,
            true, // this is symmetric
            &ziggurat_tables::ZIG_NORM_X,
            &ziggurat_tables::ZIG_NORM_F,
            pdf,
            zero_case,
        )
    }
}

/// The normal distribution `N(mean, std_dev**2)`.
///
/// This uses the ZIGNOR variant of the Ziggurat method, see [`StandardNormal`]
/// for more details.
///
/// Note that [`StandardNormal`] is an optimised implementation for mean 0, and
/// standard deviation 1.
///
/// [`StandardNormal`]: crate::distributions::StandardNormal
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Normal {
    mean: f64,
    std_dev: f64,
}

impl Normal {
    /// Construct a new `Normal` distribution with the given mean and
    /// standard deviation.
    ///
    /// # Panics
    ///
    /// Panics if `std_dev < 0`.
    #[inline]
    pub fn new(mean: f64, std_dev: f64) -> Normal {
        assert!(std_dev >= 0.0, "Normal::new called with `std_dev` < 0");
        Normal { mean, std_dev }
    }
}
impl Distribution<f64> for Normal {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let n = rng.sample(StandardNormal);
        self.mean + self.std_dev * n
    }
}


/// The log-normal distribution `ln N(mean, std_dev**2)`.
///
/// If `X` is log-normal distributed, then `ln(X)` is `N(mean, std_dev**2)`
/// distributed.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct LogNormal {
    norm: Normal,
}

impl LogNormal {
    /// Construct a new `LogNormal` distribution with the given mean
    /// and standard deviation.
    ///
    /// # Panics
    ///
    /// Panics if `std_dev < 0`.
    #[inline]
    pub fn new(mean: f64, std_dev: f64) -> LogNormal {
        assert!(std_dev >= 0.0, "LogNormal::new called with `std_dev` < 0");
        LogNormal {
            norm: Normal::new(mean, std_dev),
        }
    }
}
impl Distribution<f64> for LogNormal {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        self.norm.sample(rng).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::{LogNormal, Normal};
    use crate::distributions::Distribution;

    #[test]
    fn test_normal() {
        let norm = Normal::new(10.0, 10.0);
        let mut rng = crate::test::rng(210);
        for _ in 0..1000 {
            norm.sample(&mut rng);
        }
    }
    #[test]
    #[should_panic]
    fn test_normal_invalid_sd() {
        Normal::new(10.0, -1.0);
    }


    #[test]
    fn test_log_normal() {
        let lnorm = LogNormal::new(10.0, 10.0);
        let mut rng = crate::test::rng(211);
        for _ in 0..1000 {
            lnorm.sample(&mut rng);
        }
    }
    #[test]
    #[should_panic]
    fn test_log_normal_invalid_sd() {
        LogNormal::new(10.0, -1.0);
    }
}
