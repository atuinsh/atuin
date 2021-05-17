// Copyright 2018 Developers of the Rand project.
// Copyright 2016-2017 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The Cauchy distribution.
#![allow(deprecated)]
#![allow(clippy::all)]

use crate::distributions::Distribution;
use crate::Rng;
use std::f64::consts::PI;

/// The Cauchy distribution `Cauchy(median, scale)`.
///
/// This distribution has a density function:
/// `f(x) = 1 / (pi * scale * (1 + ((x - median) / scale)^2))`
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Cauchy {
    median: f64,
    scale: f64,
}

impl Cauchy {
    /// Construct a new `Cauchy` with the given shape parameters
    /// `median` the peak location and `scale` the scale factor.
    /// Panics if `scale <= 0`.
    pub fn new(median: f64, scale: f64) -> Cauchy {
        assert!(scale > 0.0, "Cauchy::new called with scale factor <= 0");
        Cauchy { median, scale }
    }
}

impl Distribution<f64> for Cauchy {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        // sample from [0, 1)
        let x = rng.gen::<f64>();
        // get standard cauchy random number
        // note that π/2 is not exactly representable, even if x=0.5 the result is finite
        let comp_dev = (PI * x).tan();
        // shift and scale according to parameters
        let result = self.median + self.scale * comp_dev;
        result
    }
}

#[cfg(test)]
mod test {
    use super::Cauchy;
    use crate::distributions::Distribution;

    fn median(mut numbers: &mut [f64]) -> f64 {
        sort(&mut numbers);
        let mid = numbers.len() / 2;
        numbers[mid]
    }

    fn sort(numbers: &mut [f64]) {
        numbers.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }

    #[test]
    fn test_cauchy_averages() {
        // NOTE: given that the variance and mean are undefined,
        // this test does not have any rigorous statistical meaning.
        let cauchy = Cauchy::new(10.0, 5.0);
        let mut rng = crate::test::rng(123);
        let mut numbers: [f64; 1000] = [0.0; 1000];
        let mut sum = 0.0;
        for i in 0..1000 {
            numbers[i] = cauchy.sample(&mut rng);
            sum += numbers[i];
        }
        let median = median(&mut numbers);
        println!("Cauchy median: {}", median);
        assert!((median - 10.0).abs() < 0.4); // not 100% certain, but probable enough
        let mean = sum / 1000.0;
        println!("Cauchy mean: {}", mean);
        // for a Cauchy distribution the mean should not converge
        assert!((mean - 10.0).abs() > 0.4); // not 100% certain, but probable enough
    }

    #[test]
    #[should_panic]
    fn test_cauchy_invalid_scale_zero() {
        Cauchy::new(0.0, 0.0);
    }

    #[test]
    #[should_panic]
    fn test_cauchy_invalid_scale_neg() {
        Cauchy::new(0.0, -10.0);
    }
}
