// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![feature(test)]
#![allow(non_snake_case)]

extern crate test;

const RAND_BENCH_N: u64 = 1000;
const BYTES_LEN: usize = 1024;

use std::mem::size_of;
use test::{black_box, Bencher};

use rand::prelude::*;
use rand::rngs::adapter::ReseedingRng;
use rand::rngs::{mock::StepRng, OsRng};
use rand_chacha::{ChaCha12Rng, ChaCha20Core, ChaCha20Rng, ChaCha8Rng};
use rand_hc::Hc128Rng;
use rand_pcg::{Pcg32, Pcg64, Pcg64Mcg};

macro_rules! gen_bytes {
    ($fnn:ident, $gen:expr) => {
        #[bench]
        fn $fnn(b: &mut Bencher) {
            let mut rng = $gen;
            let mut buf = [0u8; BYTES_LEN];
            b.iter(|| {
                for _ in 0..RAND_BENCH_N {
                    rng.fill_bytes(&mut buf);
                    black_box(buf);
                }
            });
            b.bytes = BYTES_LEN as u64 * RAND_BENCH_N;
        }
    };
}

gen_bytes!(gen_bytes_step, StepRng::new(0, 1));
gen_bytes!(gen_bytes_pcg32, Pcg32::from_entropy());
gen_bytes!(gen_bytes_pcg64, Pcg64::from_entropy());
gen_bytes!(gen_bytes_pcg64mcg, Pcg64Mcg::from_entropy());
gen_bytes!(gen_bytes_chacha8, ChaCha8Rng::from_entropy());
gen_bytes!(gen_bytes_chacha12, ChaCha12Rng::from_entropy());
gen_bytes!(gen_bytes_chacha20, ChaCha20Rng::from_entropy());
gen_bytes!(gen_bytes_hc128, Hc128Rng::from_entropy());
gen_bytes!(gen_bytes_std, StdRng::from_entropy());
#[cfg(feature = "small_rng")]
gen_bytes!(gen_bytes_small, SmallRng::from_entropy());
gen_bytes!(gen_bytes_os, OsRng);

macro_rules! gen_uint {
    ($fnn:ident, $ty:ty, $gen:expr) => {
        #[bench]
        fn $fnn(b: &mut Bencher) {
            let mut rng = $gen;
            b.iter(|| {
                let mut accum: $ty = 0;
                for _ in 0..RAND_BENCH_N {
                    accum = accum.wrapping_add(rng.gen::<$ty>());
                }
                accum
            });
            b.bytes = size_of::<$ty>() as u64 * RAND_BENCH_N;
        }
    };
}

gen_uint!(gen_u32_step, u32, StepRng::new(0, 1));
gen_uint!(gen_u32_pcg32, u32, Pcg32::from_entropy());
gen_uint!(gen_u32_pcg64, u32, Pcg64::from_entropy());
gen_uint!(gen_u32_pcg64mcg, u32, Pcg64Mcg::from_entropy());
gen_uint!(gen_u32_chacha8, u32, ChaCha8Rng::from_entropy());
gen_uint!(gen_u32_chacha12, u32, ChaCha12Rng::from_entropy());
gen_uint!(gen_u32_chacha20, u32, ChaCha20Rng::from_entropy());
gen_uint!(gen_u32_hc128, u32, Hc128Rng::from_entropy());
gen_uint!(gen_u32_std, u32, StdRng::from_entropy());
#[cfg(feature = "small_rng")]
gen_uint!(gen_u32_small, u32, SmallRng::from_entropy());
gen_uint!(gen_u32_os, u32, OsRng);

gen_uint!(gen_u64_step, u64, StepRng::new(0, 1));
gen_uint!(gen_u64_pcg32, u64, Pcg32::from_entropy());
gen_uint!(gen_u64_pcg64, u64, Pcg64::from_entropy());
gen_uint!(gen_u64_pcg64mcg, u64, Pcg64Mcg::from_entropy());
gen_uint!(gen_u64_chacha8, u64, ChaCha8Rng::from_entropy());
gen_uint!(gen_u64_chacha12, u64, ChaCha12Rng::from_entropy());
gen_uint!(gen_u64_chacha20, u64, ChaCha20Rng::from_entropy());
gen_uint!(gen_u64_hc128, u64, Hc128Rng::from_entropy());
gen_uint!(gen_u64_std, u64, StdRng::from_entropy());
#[cfg(feature = "small_rng")]
gen_uint!(gen_u64_small, u64, SmallRng::from_entropy());
gen_uint!(gen_u64_os, u64, OsRng);

macro_rules! init_gen {
    ($fnn:ident, $gen:ident) => {
        #[bench]
        fn $fnn(b: &mut Bencher) {
            let mut rng = Pcg32::from_entropy();
            b.iter(|| {
                let r2 = $gen::from_rng(&mut rng).unwrap();
                r2
            });
        }
    };
}

init_gen!(init_pcg32, Pcg32);
init_gen!(init_pcg64, Pcg64);
init_gen!(init_pcg64mcg, Pcg64Mcg);
init_gen!(init_hc128, Hc128Rng);
init_gen!(init_chacha, ChaCha20Rng);

const RESEEDING_BYTES_LEN: usize = 1024 * 1024;
const RESEEDING_BENCH_N: u64 = 16;

macro_rules! reseeding_bytes {
    ($fnn:ident, $thresh:expr) => {
        #[bench]
        fn $fnn(b: &mut Bencher) {
            let mut rng = ReseedingRng::new(ChaCha20Core::from_entropy(), $thresh * 1024, OsRng);
            let mut buf = [0u8; RESEEDING_BYTES_LEN];
            b.iter(|| {
                for _ in 0..RESEEDING_BENCH_N {
                    rng.fill_bytes(&mut buf);
                    black_box(&buf);
                }
            });
            b.bytes = RESEEDING_BYTES_LEN as u64 * RESEEDING_BENCH_N;
        }
    };
}

reseeding_bytes!(reseeding_chacha20_4k, 4);
reseeding_bytes!(reseeding_chacha20_16k, 16);
reseeding_bytes!(reseeding_chacha20_32k, 32);
reseeding_bytes!(reseeding_chacha20_64k, 64);
reseeding_bytes!(reseeding_chacha20_256k, 256);
reseeding_bytes!(reseeding_chacha20_1M, 1024);


macro_rules! threadrng_uint {
    ($fnn:ident, $ty:ty) => {
        #[bench]
        fn $fnn(b: &mut Bencher) {
            let mut rng = thread_rng();
            b.iter(|| {
                let mut accum: $ty = 0;
                for _ in 0..RAND_BENCH_N {
                    accum = accum.wrapping_add(rng.gen::<$ty>());
                }
                accum
            });
            b.bytes = size_of::<$ty>() as u64 * RAND_BENCH_N;
        }
    };
}

threadrng_uint!(thread_rng_u32, u32);
threadrng_uint!(thread_rng_u64, u64);
