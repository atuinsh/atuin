// Copyright 2018 Developers of the Rand project.
// Copyright 2016-2017 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The Poisson distribution.
#![allow(deprecated)]

use crate::distributions::utils::log_gamma;
use crate::distributions::{Cauchy, Distribution};
use crate::Rng;

/// The Poisson distribution `Poisson(lambda)`.
///
/// This distribution has a density function:
/// `f(k) = lambda^k * exp(-lambda) / k!` for `k >= 0`.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Poisson {
    lambda: f64,
    // precalculated values
    exp_lambda: f64,
    log_lambda: f64,
    sqrt_2lambda: f64,
    magic_val: f64,
}

impl Poisson {
    /// Construct a new `Poisson` with the given shape parameter
    /// `lambda`. Panics if `lambda <= 0`.
    pub fn new(lambda: f64) -> Poisson {
        assert!(lambda > 0.0, "Poisson::new called with lambda <= 0");
        let log_lambda = lambda.ln();
        Poisson {
            lambda,
            exp_lambda: (-lambda).exp(),
            log_lambda,
            sqrt_2lambda: (2.0 * lambda).sqrt(),
            magic_val: lambda * log_lambda - log_gamma(1.0 + lambda),
        }
    }
}

impl Distribution<u64> for Poisson {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> u64 {
        // using the algorithm from Numerical Recipes in C

        // for low expected values use the Knuth method
        if self.lambda < 12.0 {
            let mut result = 0;
            let mut p = 1.0;
            while p > self.exp_lambda {
                p *= rng.gen::<f64>();
                result += 1;
            }
            result - 1
        }
        // high expected values - rejection method
        else {
            let mut int_result: u64;

            // we use the Cauchy distribution as the comparison distribution
            // f(x) ~ 1/(1+x^2)
            let cauchy = Cauchy::new(0.0, 1.0);

            loop {
                let mut result;
                let mut comp_dev;

                loop {
                    // draw from the Cauchy distribution
                    comp_dev = rng.sample(cauchy);
                    // shift the peak of the comparison ditribution
                    result = self.sqrt_2lambda * comp_dev + self.lambda;
                    // repeat the drawing until we are in the range of possible values
                    if result >= 0.0 {
                        break;
                    }
                }
                // now the result is a random variable greater than 0 with Cauchy distribution
                // the result should be an integer value
                result = result.floor();
                int_result = result as u64;

                // this is the ratio of the Poisson distribution to the comparison distribution
                // the magic value scales the distribution function to a range of approximately 0-1
                // since it is not exact, we multiply the ratio by 0.9 to avoid ratios greater than 1
                // this doesn't change the resulting distribution, only increases the rate of failed drawings
                let check = 0.9
                    * (1.0 + comp_dev * comp_dev)
                    * (result * self.log_lambda - log_gamma(1.0 + result) - self.magic_val).exp();

                // check with uniform random value - if below the threshold, we are within the target distribution
                if rng.gen::<f64>() <= check {
                    break;
                }
            }
            int_result
        }
    }
}

#[cfg(test)]
mod test {
    use super::Poisson;
    use crate::distributions::Distribution;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri is too slow
    fn test_poisson_10() {
        let poisson = Poisson::new(10.0);
        let mut rng = crate::test::rng(123);
        let mut sum = 0;
        for _ in 0..1000 {
            sum += poisson.sample(&mut rng);
        }
        let avg = (sum as f64) / 1000.0;
        println!("Poisson average: {}", avg);
        assert!((avg - 10.0).abs() < 0.5); // not 100% certain, but probable enough
    }

    #[test]
    fn test_poisson_15() {
        // Take the 'high expected values' path
        let poisson = Poisson::new(15.0);
        let mut rng = crate::test::rng(123);
        let mut sum = 0;
        for _ in 0..1000 {
            sum += poisson.sample(&mut rng);
        }
        let avg = (sum as f64) / 1000.0;
        println!("Poisson average: {}", avg);
        assert!((avg - 15.0).abs() < 0.5); // not 100% certain, but probable enough
    }

    #[test]
    #[should_panic]
    fn test_poisson_invalid_lambda_zero() {
        Poisson::new(0.0);
    }

    #[test]
    #[should_panic]
    fn test_poisson_invalid_lambda_neg() {
        Poisson::new(-10.0);
    }
}
