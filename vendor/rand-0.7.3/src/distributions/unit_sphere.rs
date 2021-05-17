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

/// Samples uniformly from the surface of the unit sphere in three dimensions.
///
/// Implemented via a method by Marsaglia[^1].
///
/// [^1]: Marsaglia, George (1972). [*Choosing a Point from the Surface of a
///       Sphere.*](https://doi.org/10.1214/aoms/1177692644)
///       Ann. Math. Statist. 43, no. 2, 645--646.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct UnitSphereSurface;

impl UnitSphereSurface {
    /// Construct a new `UnitSphereSurface` distribution.
    #[inline]
    pub fn new() -> UnitSphereSurface {
        UnitSphereSurface
    }
}

impl Distribution<[f64; 3]> for UnitSphereSurface {
    #[inline]
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> [f64; 3] {
        let uniform = Uniform::new(-1., 1.);
        loop {
            let (x1, x2) = (uniform.sample(rng), uniform.sample(rng));
            let sum = x1 * x1 + x2 * x2;
            if sum >= 1. {
                continue;
            }
            let factor = 2. * (1.0_f64 - sum).sqrt();
            return [x1 * factor, x2 * factor, 1. - 2. * sum];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UnitSphereSurface;
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
        let dist = UnitSphereSurface::new();
        for _ in 0..1000 {
            let x = dist.sample(&mut rng);
            assert_almost_eq!(x[0] * x[0] + x[1] * x[1] + x[2] * x[2], 1., 1e-15);
        }
    }

    #[test]
    fn value_stability() {
        let mut rng = crate::test::rng(2);
        let expected = [
            [0.03247542860231647, -0.7830477442152738, 0.6211131755296027],
            [-0.09978440840914075, 0.9706650829833128, -0.21875184231323952],
            [0.2735582468624679, 0.9435374242279655, -0.1868234852870203],
        ];
        let samples = [
            UnitSphereSurface.sample(&mut rng),
            UnitSphereSurface.sample(&mut rng),
            UnitSphereSurface.sample(&mut rng),
        ];
        assert_eq!(samples, expected);
    }
}
