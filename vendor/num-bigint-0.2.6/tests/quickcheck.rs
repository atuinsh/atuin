#![cfg(feature = "quickcheck")]
#![cfg(feature = "quickcheck_macros")]

extern crate num_bigint;
extern crate num_integer;
extern crate num_traits;

extern crate quickcheck;
#[macro_use]
extern crate quickcheck_macros;

use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{Num, One, Pow, Signed, Zero};
use quickcheck::{QuickCheck, StdThreadGen, TestResult};

#[quickcheck]
fn quickcheck_unsigned_eq_reflexive(a: BigUint) -> bool {
    a == a
}

#[quickcheck]
fn quickcheck_signed_eq_reflexive(a: BigInt) -> bool {
    a == a
}

#[quickcheck]
fn quickcheck_unsigned_eq_symmetric(a: BigUint, b: BigUint) -> bool {
    if a == b {
        b == a
    } else {
        b != a
    }
}

#[quickcheck]
fn quickcheck_signed_eq_symmetric(a: BigInt, b: BigInt) -> bool {
    if a == b {
        b == a
    } else {
        b != a
    }
}

#[test]
fn quickcheck_arith_primitive() {
    let gen = StdThreadGen::new(usize::max_value());
    let mut qc = QuickCheck::with_gen(gen);

    fn test_unsigned_add_primitive(a: usize, b: usize) -> TestResult {
        let actual = BigUint::from(a) + BigUint::from(b);
        match a.checked_add(b) {
            None => TestResult::discard(),
            Some(expected) => TestResult::from_bool(BigUint::from(expected) == actual),
        }
    }

    fn test_signed_add_primitive(a: isize, b: isize) -> TestResult {
        let actual = BigInt::from(a) + BigInt::from(b);
        match a.checked_add(b) {
            None => TestResult::discard(),
            Some(expected) => TestResult::from_bool(BigInt::from(expected) == actual),
        }
    }

    fn test_unsigned_mul_primitive(a: u64, b: u64) -> bool {
        //maximum value of u64 means no overflow
        BigUint::from(a as u128 * b as u128) == BigUint::from(a) * BigUint::from(b)
    }

    fn test_signed_mul_primitive(a: i64, b: i64) -> bool {
        //maximum value of i64 means no overflow
        BigInt::from(a as i128 * b as i128) == BigInt::from(a) * BigInt::from(b)
    }

    fn test_unsigned_sub_primitive(a: u128, b: u128) -> bool {
        if b < a {
            BigUint::from(a - b) == BigUint::from(a) - BigUint::from(b)
        } else {
            BigUint::from(b - a) == BigUint::from(b) - BigUint::from(a)
        }
    }

    fn test_signed_sub_primitive(a: i128, b: i128) -> bool {
        if b < a {
            BigInt::from(a - b) == BigInt::from(a) - BigInt::from(b)
        } else {
            BigInt::from(b - a) == BigInt::from(b) - BigInt::from(a)
        }
    }

    fn test_unsigned_div_primitive(a: u128, b: u128) -> TestResult {
        if b == 0 {
            TestResult::discard()
        } else {
            TestResult::from_bool(BigUint::from(a / b) == BigUint::from(a) / BigUint::from(b))
        }
    }

    fn test_signed_div_primitive(a: i128, b: i128) -> TestResult {
        if b == 0 {
            TestResult::discard()
        } else {
            TestResult::from_bool(BigInt::from(a / b) == BigInt::from(a) / BigInt::from(b))
        }
    }

    qc.quickcheck(test_unsigned_add_primitive as fn(usize, usize) -> TestResult);
    qc.quickcheck(test_signed_add_primitive as fn(isize, isize) -> TestResult);
    qc.quickcheck(test_unsigned_mul_primitive as fn(u64, u64) -> bool);
    qc.quickcheck(test_signed_mul_primitive as fn(i64, i64) -> bool);
    qc.quickcheck(test_unsigned_sub_primitive as fn(u128, u128) -> bool);
    qc.quickcheck(test_signed_sub_primitive as fn(i128, i128) -> bool);
    qc.quickcheck(test_unsigned_div_primitive as fn(u128, u128) -> TestResult);
    qc.quickcheck(test_signed_div_primitive as fn(i128, i128) -> TestResult);
}

#[quickcheck]
fn quickcheck_unsigned_add_commutative(a: BigUint, b: BigUint) -> bool {
    &a + &b == b + a
}

#[quickcheck]
fn quickcheck_signed_add_commutative(a: BigInt, b: BigInt) -> bool {
    &a + &b == b + a
}

#[quickcheck]
fn quickcheck_unsigned_add_zero(a: BigUint) -> bool {
    a == &a + BigUint::zero()
}

#[quickcheck]
fn quickcheck_signed_add_zero(a: BigInt) -> bool {
    a == &a + BigInt::zero()
}

#[quickcheck]
fn quickcheck_unsigned_add_associative(a: BigUint, b: BigUint, c: BigUint) -> bool {
    (&a + &b) + &c == a + (b + c)
}

#[quickcheck]
fn quickcheck_signed_add_associative(a: BigInt, b: BigInt, c: BigInt) -> bool {
    (&a + &b) + &c == a + (b + c)
}

#[quickcheck]
fn quickcheck_unsigned_mul_zero(a: BigUint) -> bool {
    a * BigUint::zero() == BigUint::zero()
}

#[quickcheck]
fn quickcheck_signed_mul_zero(a: BigInt) -> bool {
    a * BigInt::zero() == BigInt::zero()
}

#[quickcheck]
fn quickcheck_unsigned_mul_one(a: BigUint) -> bool {
    &a * BigUint::one() == a
}

#[quickcheck]
fn quickcheck_signed_mul_one(a: BigInt) -> bool {
    &a * BigInt::one() == a
}

#[quickcheck]
fn quickcheck_unsigned_mul_commutative(a: BigUint, b: BigUint) -> bool {
    &a * &b == b * a
}

#[quickcheck]
fn quickcheck_signed_mul_commutative(a: BigInt, b: BigInt) -> bool {
    &a * &b == b * a
}

#[quickcheck]
fn quickcheck_unsigned_mul_associative(a: BigUint, b: BigUint, c: BigUint) -> bool {
    (&a * &b) * &c == a * (b * c)
}

#[quickcheck]
fn quickcheck_signed_mul_associative(a: BigInt, b: BigInt, c: BigInt) -> bool {
    (&a * &b) * &c == a * (b * c)
}

#[quickcheck]
fn quickcheck_unsigned_distributive(a: BigUint, b: BigUint, c: BigUint) -> bool {
    &a * (&b + &c) == &a * b + a * c
}

#[quickcheck]
fn quickcheck_signed_distributive(a: BigInt, b: BigInt, c: BigInt) -> bool {
    &a * (&b + &c) == &a * b + a * c
}

#[quickcheck]
///Tests that exactly one of a<b a>b a=b is true
fn quickcheck_unsigned_ge_le_eq_mut_exclusive(a: BigUint, b: BigUint) -> bool {
    let gt_lt_eq = vec![a > b, a < b, a == b];
    gt_lt_eq
        .iter()
        .fold(0, |acc, e| if *e { acc + 1 } else { acc })
        == 1
}

#[quickcheck]
///Tests that exactly one of a<b a>b a=b is true
fn quickcheck_signed_ge_le_eq_mut_exclusive(a: BigInt, b: BigInt) -> bool {
    let gt_lt_eq = vec![a > b, a < b, a == b];
    gt_lt_eq
        .iter()
        .fold(0, |acc, e| if *e { acc + 1 } else { acc })
        == 1
}

#[quickcheck]
/// Tests correctness of subtraction assuming addition is correct
fn quickcheck_unsigned_sub(a: BigUint, b: BigUint) -> bool {
    if b < a {
        &a - &b + b == a
    } else {
        &b - &a + a == b
    }
}

#[quickcheck]
/// Tests correctness of subtraction assuming addition is correct
fn quickcheck_signed_sub(a: BigInt, b: BigInt) -> bool {
    if b < a {
        &a - &b + b == a
    } else {
        &b - &a + a == b
    }
}

#[quickcheck]
fn quickcheck_unsigned_pow_zero(a: BigUint) -> bool {
    a.pow(0_u32) == BigUint::one()
}

#[quickcheck]
fn quickcheck_unsigned_pow_one(a: BigUint) -> bool {
    a.pow(1_u32) == a
}

#[quickcheck]
fn quickcheck_unsigned_sqrt(a: BigUint) -> bool {
    (&a * &a).sqrt() == a
}

#[quickcheck]
fn quickcheck_unsigned_cbrt(a: BigUint) -> bool {
    (&a * &a * &a).cbrt() == a
}

#[quickcheck]
fn quickcheck_signed_cbrt(a: BigInt) -> bool {
    (&a * &a * &a).cbrt() == a
}

#[quickcheck]
fn quickcheck_unsigned_conversion(a: BigUint, radix: u8) -> TestResult {
    let radix = radix as u32;
    if radix > 36 || radix < 2 {
        return TestResult::discard();
    }
    let string = a.to_str_radix(radix);
    TestResult::from_bool(a == BigUint::from_str_radix(&string, radix).unwrap())
}

#[quickcheck]
fn quickcheck_signed_conversion(a: BigInt, radix: u8) -> TestResult {
    let radix = radix as u32;
    if radix > 36 || radix < 2 {
        return TestResult::discard();
    }
    let string = a.to_str_radix(radix);
    TestResult::from_bool(a == BigInt::from_str_radix(&string, radix).unwrap())
}

#[test]
fn quicktest_shift() {
    let gen = StdThreadGen::new(usize::max_value());
    let mut qc = QuickCheck::with_gen(gen);

    fn test_shr_unsigned(a: u64, shift: u8) -> TestResult {
        let shift = (shift % 64) as usize; //shift at most 64 bits
        let big_a = BigUint::from(a);
        TestResult::from_bool(BigUint::from(a >> shift) == big_a >> shift)
    }

    fn test_shr_signed(a: i64, shift: u8) -> TestResult {
        let shift = (shift % 64) as usize; //shift at most 64 bits
        let big_a = BigInt::from(a);
        TestResult::from_bool(BigInt::from(a >> shift) == big_a >> shift)
    }

    fn test_shl_unsigned(a: u32, shift: u8) -> TestResult {
        let shift = (shift % 32) as usize; //shift at most 32 bits
        let a = a as u64; //leave room for the shifted bits
        let big_a = BigUint::from(a);
        TestResult::from_bool(BigUint::from(a >> shift) == big_a >> shift)
    }

    fn test_shl_signed(a: i32, shift: u8) -> TestResult {
        let shift = (shift % 32) as usize;
        let a = a as u64; //leave room for the shifted bits
        let big_a = BigInt::from(a);
        TestResult::from_bool(BigInt::from(a >> shift) == big_a >> shift)
    }

    qc.quickcheck(test_shr_unsigned as fn(u64, u8) -> TestResult);
    qc.quickcheck(test_shr_signed as fn(i64, u8) -> TestResult);
    qc.quickcheck(test_shl_unsigned as fn(u32, u8) -> TestResult);
    qc.quickcheck(test_shl_signed as fn(i32, u8) -> TestResult);
}

#[test]
fn quickcheck_modpow() {
    let gen = StdThreadGen::new(usize::max_value());
    let mut qc = QuickCheck::with_gen(gen);

    fn simple_modpow(base: &BigInt, exponent: &BigInt, modulus: &BigInt) -> BigInt {
        assert!(!exponent.is_negative());
        let mut result = BigInt::one().mod_floor(modulus);
        let mut base = base.mod_floor(modulus);
        let mut exponent = exponent.clone();
        while !exponent.is_zero() {
            if exponent.is_odd() {
                result = (result * &base).mod_floor(modulus);
            }
            base = (&base * &base).mod_floor(modulus);
            exponent >>= 1;
        }
        result
    }

    fn test_modpow(base: i128, exponent: u128, modulus: i128) -> TestResult {
        if modulus.is_zero() {
            TestResult::discard()
        } else {
            let base = BigInt::from(base);
            let exponent = BigInt::from(exponent);
            let modulus = BigInt::from(modulus);
            let modpow = base.modpow(&exponent, &modulus);
            let simple = simple_modpow(&base, &exponent, &modulus);
            if modpow != simple {
                eprintln!("{}.modpow({}, {})", base, exponent, modulus);
                eprintln!("  expected {}", simple);
                eprintln!("    actual {}", modpow);
                TestResult::failed()
            } else {
                TestResult::passed()
            }
        }
    }

    qc.quickcheck(test_modpow as fn(i128, u128, i128) -> TestResult);
}
