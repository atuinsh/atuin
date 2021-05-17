// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The Weibull distribution.
#![allow(deprecated)]

use crate::distributions::{Distribution, OpenClosed01};
use crate::Rng;

/// Samples floating-point numbers according to the Weibull distribution
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Weibull {
    inv_shape: f64,
    scale: f64,
}

impl Weibull {
    /// Construct a new `Weibull` distribution with given `scale` and `shape`.
    ///
    /// # Panics
    ///
    /// `scale` and `shape` have to be non-zero and positive.
    pub fn new(scale: f64, shape: f64) -> Weibull {
        assert!((scale > 0.) & (shape > 0.));
        Weibull {
            inv_shape: 1. / shape,
            scale,
        }
    }
}

impl Distribution<f64> for Weibull {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let x: f64 = rng.sample(OpenClosed01);
        self.scale * (-x.ln()).powf(self.inv_shape)
    }
}

#[cfg(test)]
mod tests {
    use super::Weibull;
    use crate::distributions::Distribution;

    #[test]
    #[should_panic]
    fn invalid() {
        Weibull::new(0., 0.);
    }

    #[test]
    fn sample() {
        let scale = 1.0;
        let shape = 2.0;
        let d = Weibull::new(scale, shape);
        let mut rng = crate::test::rng(1);
        for _ in 0..1000 {
            let r = d.sample(&mut rng);
            assert!(r >= 0.);
        }
    }
}
