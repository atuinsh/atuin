// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The triangular distribution.
#![allow(deprecated)]

use crate::distributions::{Distribution, Standard};
use crate::Rng;

/// The triangular distribution.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Triangular {
    min: f64,
    max: f64,
    mode: f64,
}

impl Triangular {
    /// Construct a new `Triangular` with minimum `min`, maximum `max` and mode
    /// `mode`.
    ///
    /// # Panics
    ///
    /// If `max < mode`, `mode < max` or `max == min`.
    #[inline]
    pub fn new(min: f64, max: f64, mode: f64) -> Triangular {
        assert!(max >= mode);
        assert!(mode >= min);
        assert!(max != min);
        Triangular { min, max, mode }
    }
}

impl Distribution<f64> for Triangular {
    #[inline]
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let f: f64 = rng.sample(Standard);
        let diff_mode_min = self.mode - self.min;
        let diff_max_min = self.max - self.min;
        if f * diff_max_min < diff_mode_min {
            self.min + (f * diff_max_min * diff_mode_min).sqrt()
        } else {
            self.max - ((1. - f) * diff_max_min * (self.max - self.mode)).sqrt()
        }
    }
}

#[cfg(test)]
mod test {
    use super::Triangular;
    use crate::distributions::Distribution;

    #[test]
    fn test_new() {
        for &(min, max, mode) in &[
            (-1., 1., 0.),
            (1., 2., 1.),
            (5., 25., 25.),
            (1e-5, 1e5, 1e-3),
            (0., 1., 0.9),
            (-4., -0.5, -2.),
            (-13.039, 8.41, 1.17),
        ] {
            println!("{} {} {}", min, max, mode);
            let _ = Triangular::new(min, max, mode);
        }
    }

    #[test]
    fn test_sample() {
        let norm = Triangular::new(0., 1., 0.5);
        let mut rng = crate::test::rng(1);
        for _ in 0..1000 {
            norm.sample(&mut rng);
        }
    }
}
