// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The Pareto distribution.
#![allow(deprecated)]

use crate::distributions::{Distribution, OpenClosed01};
use crate::Rng;

/// Samples floating-point numbers according to the Pareto distribution
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Pareto {
    scale: f64,
    inv_neg_shape: f64,
}

impl Pareto {
    /// Construct a new Pareto distribution with given `scale` and `shape`.
    ///
    /// In the literature, `scale` is commonly written as x<sub>m</sub> or k and
    /// `shape` is often written as α.
    ///
    /// # Panics
    ///
    /// `scale` and `shape` have to be non-zero and positive.
    pub fn new(scale: f64, shape: f64) -> Pareto {
        assert!((scale > 0.) & (shape > 0.));
        Pareto {
            scale,
            inv_neg_shape: -1.0 / shape,
        }
    }
}

impl Distribution<f64> for Pareto {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let u: f64 = rng.sample(OpenClosed01);
        self.scale * u.powf(self.inv_neg_shape)
    }
}

#[cfg(test)]
mod tests {
    use super::Pareto;
    use crate::distributions::Distribution;

    #[test]
    #[should_panic]
    fn invalid() {
        Pareto::new(0., 0.);
    }

    #[test]
    fn sample() {
        let scale = 1.0;
        let shape = 2.0;
        let d = Pareto::new(scale, shape);
        let mut rng = crate::test::rng(1);
        for _ in 0..1000 {
            let r = d.sample(&mut rng);
            assert!(r >= scale);
        }
    }
}
