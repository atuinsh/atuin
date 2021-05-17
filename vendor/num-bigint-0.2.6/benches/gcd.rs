#![feature(test)]
#![cfg(feature = "rand")]

extern crate num_bigint;
extern crate num_integer;
extern crate num_traits;
extern crate rand;
extern crate test;

use num_bigint::{BigUint, RandBigInt};
use num_integer::Integer;
use num_traits::Zero;
use rand::{SeedableRng, StdRng};
use test::Bencher;

fn get_rng() -> StdRng {
    let mut seed = [0; 32];
    for i in 1..32 {
        seed[usize::from(i)] = i;
    }
    SeedableRng::from_seed(seed)
}

fn bench(b: &mut Bencher, bits: usize, gcd: fn(&BigUint, &BigUint) -> BigUint) {
    let mut rng = get_rng();
    let x = rng.gen_biguint(bits);
    let y = rng.gen_biguint(bits);

    assert_eq!(euclid(&x, &y), x.gcd(&y));

    b.iter(|| gcd(&x, &y));
}

fn euclid(x: &BigUint, y: &BigUint) -> BigUint {
    // Use Euclid's algorithm
    let mut m = x.clone();
    let mut n = y.clone();
    while !m.is_zero() {
        let temp = m;
        m = n % &temp;
        n = temp;
    }
    return n;
}

#[bench]
fn gcd_euclid_0064(b: &mut Bencher) {
    bench(b, 64, euclid);
}

#[bench]
fn gcd_euclid_0256(b: &mut Bencher) {
    bench(b, 256, euclid);
}

#[bench]
fn gcd_euclid_1024(b: &mut Bencher) {
    bench(b, 1024, euclid);
}

#[bench]
fn gcd_euclid_4096(b: &mut Bencher) {
    bench(b, 4096, euclid);
}

// Integer for BigUint now uses Stein for gcd

#[bench]
fn gcd_stein_0064(b: &mut Bencher) {
    bench(b, 64, BigUint::gcd);
}

#[bench]
fn gcd_stein_0256(b: &mut Bencher) {
    bench(b, 256, BigUint::gcd);
}

#[bench]
fn gcd_stein_1024(b: &mut Bencher) {
    bench(b, 1024, BigUint::gcd);
}

#[bench]
fn gcd_stein_4096(b: &mut Bencher) {
    bench(b, 4096, BigUint::gcd);
}
