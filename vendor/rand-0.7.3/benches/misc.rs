// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![feature(test)]

extern crate test;

const RAND_BENCH_N: u64 = 1000;

use test::Bencher;

use rand::distributions::{Bernoulli, Distribution, Standard};
use rand::prelude::*;
use rand_pcg::{Pcg32, Pcg64Mcg};

#[bench]
fn misc_gen_bool_const(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let mut accum = true;
        for _ in 0..crate::RAND_BENCH_N {
            accum ^= rng.gen_bool(0.18);
        }
        accum
    })
}

#[bench]
fn misc_gen_bool_var(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let mut accum = true;
        let mut p = 0.18;
        for _ in 0..crate::RAND_BENCH_N {
            accum ^= rng.gen_bool(p);
            p += 0.0001;
        }
        accum
    })
}

#[bench]
fn misc_gen_ratio_const(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let mut accum = true;
        for _ in 0..crate::RAND_BENCH_N {
            accum ^= rng.gen_ratio(2, 3);
        }
        accum
    })
}

#[bench]
fn misc_gen_ratio_var(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let mut accum = true;
        for i in 2..(crate::RAND_BENCH_N as u32 + 2) {
            accum ^= rng.gen_ratio(i, i + 1);
        }
        accum
    })
}

#[bench]
fn misc_bernoulli_const(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let d = rand::distributions::Bernoulli::new(0.18).unwrap();
        let mut accum = true;
        for _ in 0..crate::RAND_BENCH_N {
            accum ^= rng.sample(d);
        }
        accum
    })
}

#[bench]
fn misc_bernoulli_var(b: &mut Bencher) {
    let mut rng = Pcg32::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let mut accum = true;
        let mut p = 0.18;
        for _ in 0..crate::RAND_BENCH_N {
            let d = Bernoulli::new(p).unwrap();
            accum ^= rng.sample(d);
            p += 0.0001;
        }
        accum
    })
}

#[bench]
fn gen_1k_iter_repeat(b: &mut Bencher) {
    use std::iter;
    let mut rng = Pcg64Mcg::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let v: Vec<u64> = iter::repeat(()).map(|()| rng.gen()).take(128).collect();
        v
    });
    b.bytes = 1024;
}

#[bench]
fn gen_1k_sample_iter(b: &mut Bencher) {
    let mut rng = Pcg64Mcg::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        let v: Vec<u64> = Standard.sample_iter(&mut rng).take(128).collect();
        v
    });
    b.bytes = 1024;
}

#[bench]
fn gen_1k_gen_array(b: &mut Bencher) {
    let mut rng = Pcg64Mcg::from_rng(&mut thread_rng()).unwrap();
    b.iter(|| {
        // max supported array length is 32!
        let v: [[u64; 32]; 4] = rng.gen();
        v
    });
    b.bytes = 1024;
}

#[bench]
fn gen_1k_fill(b: &mut Bencher) {
    let mut rng = Pcg64Mcg::from_rng(&mut thread_rng()).unwrap();
    let mut buf = [0u64; 128];
    b.iter(|| {
        rng.fill(&mut buf[..]);
        buf
    });
    b.bytes = 1024;
}
