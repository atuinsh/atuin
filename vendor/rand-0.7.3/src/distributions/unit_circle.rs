// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![allow(deprecated)]
#![allow(clippy::all)]

use crate::distributions::{Distribution, Uniform};
use crate::Rng;

/// Samples uniformly from the edge of the unit circle in two dimensions.
///
/// Implemented via a method by von Neumann[^1].
///
/// [^1]: von Neumann, J. (1951) [*Various Techniques Used in Connection with
///       Random Digits.*](https://mcnp.lanl.gov/pdf_files/nbs_vonneumann.pdf)
///       NBS Appl. Math. Ser., No. 12. Washington, DC: U.S. Government Printing
///       Office, pp. 36-38.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct UnitCircle;

impl UnitCircle {
    /// Construct a new `UnitCircle` distribution.
    #[inline]
    pub fn new() -> UnitCircle {
        UnitCircle
    }
}

impl Distribution<[f64; 2]> for UnitCircle {
    #[inline]
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> [f64; 2] {
        let uniform = Uniform::new(-1., 1.);
        let mut x1;
        let mut x2;
        let mut sum;
        loop {
            x1 = uniform.sample(rng);
            x2 = uniform.sample(rng);
            sum = x1 * x1 + x2 * x2;
            if sum < 1. {
                break;
            }
        }
        let diff = x1 * x1 - x2 * x2;
        [diff / sum, 2. * x1 * x2 / sum]
    }
}

#[cfg(test)]
mod tests {
    use super::UnitCircle;
    use crate::distributions::Distribution;

    /// Assert that two numbers are almost equal to each other.
    ///
    /// On panic, this macro will print the values of the expressions with their
    /// debug representations.
    macro_rules! assert_almost_eq {
        ($a:expr, $b:expr, $prec:expr) => {
            let diff = ($a - $b).abs();
            if diff > $prec {
                panic!(format!(
                    "assertion failed: `abs(left - right) = {:.1e} < {:e}`, \
                     (left: `{}`, right: `{}`)",
                    diff, $prec, $a, $b
                ));
            }
        };
    }

    #[test]
    fn norm() {
        let mut rng = crate::test::rng(1);
        let dist = UnitCircle::new();
        for _ in 0..1000 {
            let x = dist.sample(&mut rng);
            assert_almost_eq!(x[0] * x[0] + x[1] * x[1], 1., 1e-15);
        }
    }

    #[test]
    fn value_stability() {
        let mut rng = crate::test::rng(2);
        let expected = [
            [-0.9965658683520504, -0.08280380447614634],
            [-0.9790853270389644, -0.20345004884984505],
            [-0.8449189758898707, 0.5348943112253227],
        ];
        let samples = [
            UnitCircle.sample(&mut rng),
            UnitCircle.sample(&mut rng),
            UnitCircle.sample(&mut rng),
        ];
        assert_eq!(samples, expected);
    }
}
