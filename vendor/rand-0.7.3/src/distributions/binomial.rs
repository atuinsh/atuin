// Copyright 2018 Developers of the Rand project.
// Copyright 2016-2017 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The binomial distribution.
#![allow(deprecated)]
#![allow(clippy::all)]

use crate::distributions::{Distribution, Uniform};
use crate::Rng;

/// The binomial distribution `Binomial(n, p)`.
///
/// This distribution has density function:
/// `f(k) = n!/(k! (n-k)!) p^k (1-p)^(n-k)` for `k >= 0`.
#[deprecated(since = "0.7.0", note = "moved to rand_distr crate")]
#[derive(Clone, Copy, Debug)]
pub struct Binomial {
    /// Number of trials.
    n: u64,
    /// Probability of success.
    p: f64,
}

impl Binomial {
    /// Construct a new `Binomial` with the given shape parameters `n` (number
    /// of trials) and `p` (probability of success).
    ///
    /// Panics if `p < 0` or `p > 1`.
    pub fn new(n: u64, p: f64) -> Binomial {
        assert!(p >= 0.0, "Binomial::new called with p < 0");
        assert!(p <= 1.0, "Binomial::new called with p > 1");
        Binomial { n, p }
    }
}

/// Convert a `f64` to an `i64`, panicing on overflow.
// In the future (Rust 1.34), this might be replaced with `TryFrom`.
fn f64_to_i64(x: f64) -> i64 {
    assert!(x < (::std::i64::MAX as f64));
    x as i64
}

impl Distribution<u64> for Binomial {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> u64 {
        // Handle these values directly.
        if self.p == 0.0 {
            return 0;
        } else if self.p == 1.0 {
            return self.n;
        }

        // The binomial distribution is symmetrical with respect to p -> 1-p,
        // k -> n-k switch p so that it is less than 0.5 - this allows for lower
        // expected values we will just invert the result at the end
        let p = if self.p <= 0.5 { self.p } else { 1.0 - self.p };

        let result;
        let q = 1. - p;

        // For small n * min(p, 1 - p), the BINV algorithm based on the inverse
        // transformation of the binomial distribution is efficient. Otherwise,
        // the BTPE algorithm is used.
        //
        // Voratas Kachitvichyanukul and Bruce W. Schmeiser. 1988. Binomial
        // random variate generation. Commun. ACM 31, 2 (February 1988),
        // 216-222. http://dx.doi.org/10.1145/42372.42381

        // Threshold for prefering the BINV algorithm. The paper suggests 10,
        // Ranlib uses 30, and GSL uses 14.
        const BINV_THRESHOLD: f64 = 10.;

        if (self.n as f64) * p < BINV_THRESHOLD && self.n <= (::std::i32::MAX as u64) {
            // Use the BINV algorithm.
            let s = p / q;
            let a = ((self.n + 1) as f64) * s;
            let mut r = q.powi(self.n as i32);
            let mut u: f64 = rng.gen();
            let mut x = 0;
            while u > r as f64 {
                u -= r;
                x += 1;
                r *= a / (x as f64) - s;
            }
            result = x;
        } else {
            // Use the BTPE algorithm.

            // Threshold for using the squeeze algorithm. This can be freely
            // chosen based on performance. Ranlib and GSL use 20.
            const SQUEEZE_THRESHOLD: i64 = 20;

            // Step 0: Calculate constants as functions of `n` and `p`.
            let n = self.n as f64;
            let np = n * p;
            let npq = np * q;
            let f_m = np + p;
            let m = f64_to_i64(f_m);
            // radius of triangle region, since height=1 also area of region
            let p1 = (2.195 * npq.sqrt() - 4.6 * q).floor() + 0.5;
            // tip of triangle
            let x_m = (m as f64) + 0.5;
            // left edge of triangle
            let x_l = x_m - p1;
            // right edge of triangle
            let x_r = x_m + p1;
            let c = 0.134 + 20.5 / (15.3 + (m as f64));
            // p1 + area of parallelogram region
            let p2 = p1 * (1. + 2. * c);

            fn lambda(a: f64) -> f64 {
                a * (1. + 0.5 * a)
            }

            let lambda_l = lambda((f_m - x_l) / (f_m - x_l * p));
            let lambda_r = lambda((x_r - f_m) / (x_r * q));
            // p1 + area of left tail
            let p3 = p2 + c / lambda_l;
            // p1 + area of right tail
            let p4 = p3 + c / lambda_r;

            // return value
            let mut y: i64;

            let gen_u = Uniform::new(0., p4);
            let gen_v = Uniform::new(0., 1.);

            loop {
                // Step 1: Generate `u` for selecting the region. If region 1 is
                // selected, generate a triangularly distributed variate.
                let u = gen_u.sample(rng);
                let mut v = gen_v.sample(rng);
                if !(u > p1) {
                    y = f64_to_i64(x_m - p1 * v + u);
                    break;
                }

                if !(u > p2) {
                    // Step 2: Region 2, parallelograms. Check if region 2 is
                    // used. If so, generate `y`.
                    let x = x_l + (u - p1) / c;
                    v = v * c + 1.0 - (x - x_m).abs() / p1;
                    if v > 1. {
                        continue;
                    } else {
                        y = f64_to_i64(x);
                    }
                } else if !(u > p3) {
                    // Step 3: Region 3, left exponential tail.
                    y = f64_to_i64(x_l + v.ln() / lambda_l);
                    if y < 0 {
                        continue;
                    } else {
                        v *= (u - p2) * lambda_l;
                    }
                } else {
                    // Step 4: Region 4, right exponential tail.
                    y = f64_to_i64(x_r - v.ln() / lambda_r);
                    if y > 0 && (y as u64) > self.n {
                        continue;
                    } else {
                        v *= (u - p3) * lambda_r;
                    }
                }

                // Step 5: Acceptance/rejection comparison.

                // Step 5.0: Test for appropriate method of evaluating f(y).
                let k = (y - m).abs();
                if !(k > SQUEEZE_THRESHOLD && (k as f64) < 0.5 * npq - 1.) {
                    // Step 5.1: Evaluate f(y) via the recursive relationship. Start the
                    // search from the mode.
                    let s = p / q;
                    let a = s * (n + 1.);
                    let mut f = 1.0;
                    if m < y {
                        let mut i = m;
                        loop {
                            i += 1;
                            f *= a / (i as f64) - s;
                            if i == y {
                                break;
                            }
                        }
                    } else if m > y {
                        let mut i = y;
                        loop {
                            i += 1;
                            f /= a / (i as f64) - s;
                            if i == m {
                                break;
                            }
                        }
                    }
                    if v > f {
                        continue;
                    } else {
                        break;
                    }
                }

                // Step 5.2: Squeezing. Check the value of ln(v) againts upper and
                // lower bound of ln(f(y)).
                let k = k as f64;
                let rho = (k / npq) * ((k * (k / 3. + 0.625) + 1. / 6.) / npq + 0.5);
                let t = -0.5 * k * k / npq;
                let alpha = v.ln();
                if alpha < t - rho {
                    break;
                }
                if alpha > t + rho {
                    continue;
                }

                // Step 5.3: Final acceptance/rejection test.
                let x1 = (y + 1) as f64;
                let f1 = (m + 1) as f64;
                let z = (f64_to_i64(n) + 1 - m) as f64;
                let w = (f64_to_i64(n) - y + 1) as f64;

                fn stirling(a: f64) -> f64 {
                    let a2 = a * a;
                    (13860. - (462. - (132. - (99. - 140. / a2) / a2) / a2) / a2) / a / 166320.
                }

                if alpha
                        > x_m * (f1 / x1).ln()
                        + (n - (m as f64) + 0.5) * (z / w).ln()
                        + ((y - m) as f64) * (w * p / (x1 * q)).ln()
                        // We use the signs from the GSL implementation, which are
                        // different than the ones in the reference. According to
                        // the GSL authors, the new signs were verified to be
                        // correct by one of the original designers of the
                        // algorithm.
                        + stirling(f1)
                        + stirling(z)
                        - stirling(x1)
                        - stirling(w)
                {
                    continue;
                }

                break;
            }
            assert!(y >= 0);
            result = y as u64;
        }

        // Invert the result for p < 0.5.
        if p != self.p {
            self.n - result
        } else {
            result
        }
    }
}

#[cfg(test)]
mod test {
    use super::Binomial;
    use crate::distributions::Distribution;
    use crate::Rng;

    fn test_binomial_mean_and_variance<R: Rng>(n: u64, p: f64, rng: &mut R) {
        let binomial = Binomial::new(n, p);

        let expected_mean = n as f64 * p;
        let expected_variance = n as f64 * p * (1.0 - p);

        let mut results = [0.0; 1000];
        for i in results.iter_mut() {
            *i = binomial.sample(rng) as f64;
        }

        let mean = results.iter().sum::<f64>() / results.len() as f64;
        assert!(
            (mean as f64 - expected_mean).abs() < expected_mean / 50.0,
            "mean: {}, expected_mean: {}",
            mean,
            expected_mean
        );

        let variance =
            results.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / results.len() as f64;
        assert!(
            (variance - expected_variance).abs() < expected_variance / 10.0,
            "variance: {}, expected_variance: {}",
            variance,
            expected_variance
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri is too slow
    fn test_binomial() {
        let mut rng = crate::test::rng(351);
        test_binomial_mean_and_variance(150, 0.1, &mut rng);
        test_binomial_mean_and_variance(70, 0.6, &mut rng);
        test_binomial_mean_and_variance(40, 0.5, &mut rng);
        test_binomial_mean_and_variance(20, 0.7, &mut rng);
        test_binomial_mean_and_variance(20, 0.5, &mut rng);
    }

    #[test]
    fn test_binomial_end_points() {
        let mut rng = crate::test::rng(352);
        assert_eq!(rng.sample(Binomial::new(20, 0.0)), 0);
        assert_eq!(rng.sample(Binomial::new(20, 1.0)), 20);
    }

    #[test]
    #[should_panic]
    fn test_binomial_invalid_lambda_neg() {
        Binomial::new(20, -10.0);
    }
}
