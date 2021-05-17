// Copyright 2018 Developers of the Rand project.
// Copyright 2013-2018 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! # Monte Carlo estimation of π
//!
//! Imagine that we have a square with sides of length 2 and a unit circle
//! (radius = 1), both centered at the origin. The areas are:
//!
//! ```text
//!     area of circle  = πr² = π * r * r = π
//!     area of square  = 2² = 4
//! ```
//!
//! The circle is entirely within the square, so if we sample many points
//! randomly from the square, roughly π / 4 of them should be inside the circle.
//!
//! We can use the above fact to estimate the value of π: pick many points in
//! the square at random, calculate the fraction that fall within the circle,
//! and multiply this fraction by 4.

#![cfg(feature = "std")]

use rand::distributions::{Distribution, Uniform};

fn main() {
    let range = Uniform::new(-1.0f64, 1.0);
    let mut rng = rand::thread_rng();

    let total = 1_000_000;
    let mut in_circle = 0;

    for _ in 0..total {
        let a = range.sample(&mut rng);
        let b = range.sample(&mut rng);
        if a * a + b * b <= 1.0 {
            in_circle += 1;
        }
    }

    // prints something close to 3.14159...
    println!(
        "π is approximately {}",
        4. * (in_circle as f64) / (total as f64)
    );
}
