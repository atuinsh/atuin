#[allow(deprecated, unused_imports)]
use std::ascii::AsciiExt;
use std::borrow::Cow;
use std::cmp;
use std::cmp::Ordering::{self, Equal, Greater, Less};
use std::default::Default;
use std::fmt;
use std::iter::{Product, Sum};
use std::mem;
use std::ops::{
    Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div, DivAssign,
    Mul, MulAssign, Neg, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub, SubAssign,
};
use std::str::{self, FromStr};
use std::{f32, f64};
use std::{u64, u8};

#[cfg(feature = "serde")]
use serde;

use integer::{Integer, Roots};
use traits::{
    CheckedAdd, CheckedDiv, CheckedMul, CheckedSub, Float, FromPrimitive, Num, One, Pow,
    ToPrimitive, Unsigned, Zero,
};

use big_digit::{self, BigDigit};

#[path = "algorithms.rs"]
mod algorithms;
#[path = "monty.rs"]
mod monty;

use self::algorithms::{__add2, __sub2rev, add2, sub2, sub2rev};
use self::algorithms::{biguint_shl, biguint_shr};
use self::algorithms::{cmp_slice, fls, ilog2};
use self::algorithms::{div_rem, div_rem_digit, div_rem_ref, rem_digit};
use self::algorithms::{mac_with_carry, mul3, scalar_mul};
use self::monty::monty_modpow;

use UsizePromotion;

use ParseBigIntError;

#[cfg(feature = "quickcheck")]
use quickcheck::{Arbitrary, Gen};

/// A big unsigned integer type.
#[derive(Clone, Debug, Hash)]
pub struct BigUint {
    data: Vec<BigDigit>,
}

#[cfg(feature = "quickcheck")]
impl Arbitrary for BigUint {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
        // Use arbitrary from Vec
        Self::new(Vec::<u32>::arbitrary(g))
    }

    #[allow(bare_trait_objects)] // `dyn` needs Rust 1.27 to parse, even when cfg-disabled
    fn shrink(&self) -> Box<Iterator<Item = Self>> {
        // Use shrinker from Vec
        Box::new(self.data.shrink().map(BigUint::new))
    }
}

impl PartialEq for BigUint {
    #[inline]
    fn eq(&self, other: &BigUint) -> bool {
        match self.cmp(other) {
            Equal => true,
            _ => false,
        }
    }
}
impl Eq for BigUint {}

impl PartialOrd for BigUint {
    #[inline]
    fn partial_cmp(&self, other: &BigUint) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigUint {
    #[inline]
    fn cmp(&self, other: &BigUint) -> Ordering {
        cmp_slice(&self.data[..], &other.data[..])
    }
}

impl Default for BigUint {
    #[inline]
    fn default() -> BigUint {
        Zero::zero()
    }
}

impl fmt::Display for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad_integral(true, "", &self.to_str_radix(10))
    }
}

impl fmt::LowerHex for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad_integral(true, "0x", &self.to_str_radix(16))
    }
}

impl fmt::UpperHex for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = self.to_str_radix(16);
        s.make_ascii_uppercase();
        f.pad_integral(true, "0x", &s)
    }
}

impl fmt::Binary for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad_integral(true, "0b", &self.to_str_radix(2))
    }
}

impl fmt::Octal for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad_integral(true, "0o", &self.to_str_radix(8))
    }
}

impl FromStr for BigUint {
    type Err = ParseBigIntError;

    #[inline]
    fn from_str(s: &str) -> Result<BigUint, ParseBigIntError> {
        BigUint::from_str_radix(s, 10)
    }
}

// Convert from a power of two radix (bits == ilog2(radix)) where bits evenly divides
// BigDigit::BITS
fn from_bitwise_digits_le(v: &[u8], bits: usize) -> BigUint {
    debug_assert!(!v.is_empty() && bits <= 8 && big_digit::BITS % bits == 0);
    debug_assert!(v.iter().all(|&c| BigDigit::from(c) < (1 << bits)));

    let digits_per_big_digit = big_digit::BITS / bits;

    let data = v
        .chunks(digits_per_big_digit)
        .map(|chunk| {
            chunk
                .iter()
                .rev()
                .fold(0, |acc, &c| (acc << bits) | BigDigit::from(c))
        })
        .collect();

    BigUint::new(data)
}

// Convert from a power of two radix (bits == ilog2(radix)) where bits doesn't evenly divide
// BigDigit::BITS
fn from_inexact_bitwise_digits_le(v: &[u8], bits: usize) -> BigUint {
    debug_assert!(!v.is_empty() && bits <= 8 && big_digit::BITS % bits != 0);
    debug_assert!(v.iter().all(|&c| BigDigit::from(c) < (1 << bits)));

    let big_digits = (v.len() * bits + big_digit::BITS - 1) / big_digit::BITS;
    let mut data = Vec::with_capacity(big_digits);

    let mut d = 0;
    let mut dbits = 0; // number of bits we currently have in d

    // walk v accumululating bits in d; whenever we accumulate big_digit::BITS in d, spit out a
    // big_digit:
    for &c in v {
        d |= BigDigit::from(c) << dbits;
        dbits += bits;

        if dbits >= big_digit::BITS {
            data.push(d);
            dbits -= big_digit::BITS;
            // if dbits was > big_digit::BITS, we dropped some of the bits in c (they couldn't fit
            // in d) - grab the bits we lost here:
            d = BigDigit::from(c) >> (bits - dbits);
        }
    }

    if dbits > 0 {
        debug_assert!(dbits < big_digit::BITS);
        data.push(d as BigDigit);
    }

    BigUint::new(data)
}

// Read little-endian radix digits
fn from_radix_digits_be(v: &[u8], radix: u32) -> BigUint {
    debug_assert!(!v.is_empty() && !radix.is_power_of_two());
    debug_assert!(v.iter().all(|&c| u32::from(c) < radix));

    // Estimate how big the result will be, so we can pre-allocate it.
    let bits = f64::from(radix).log2() * v.len() as f64;
    let big_digits = (bits / big_digit::BITS as f64).ceil();
    let mut data = Vec::with_capacity(big_digits as usize);

    let (base, power) = get_radix_base(radix);
    let radix = radix as BigDigit;

    let r = v.len() % power;
    let i = if r == 0 { power } else { r };
    let (head, tail) = v.split_at(i);

    let first = head
        .iter()
        .fold(0, |acc, &d| acc * radix + BigDigit::from(d));
    data.push(first);

    debug_assert!(tail.len() % power == 0);
    for chunk in tail.chunks(power) {
        if data.last() != Some(&0) {
            data.push(0);
        }

        let mut carry = 0;
        for d in data.iter_mut() {
            *d = mac_with_carry(0, *d, base, &mut carry);
        }
        debug_assert!(carry == 0);

        let n = chunk
            .iter()
            .fold(0, |acc, &d| acc * radix + BigDigit::from(d));
        add2(&mut data, &[n]);
    }

    BigUint::new(data)
}

impl Num for BigUint {
    type FromStrRadixErr = ParseBigIntError;

    /// Creates and initializes a `BigUint`.
    fn from_str_radix(s: &str, radix: u32) -> Result<BigUint, ParseBigIntError> {
        assert!(2 <= radix && radix <= 36, "The radix must be within 2...36");
        let mut s = s;
        if s.starts_with('+') {
            let tail = &s[1..];
            if !tail.starts_with('+') {
                s = tail
            }
        }

        if s.is_empty() {
            return Err(ParseBigIntError::empty());
        }

        if s.starts_with('_') {
            // Must lead with a real digit!
            return Err(ParseBigIntError::invalid());
        }

        // First normalize all characters to plain digit values
        let mut v = Vec::with_capacity(s.len());
        for b in s.bytes() {
            #[allow(unknown_lints, ellipsis_inclusive_range_patterns)]
            let d = match b {
                b'0'...b'9' => b - b'0',
                b'a'...b'z' => b - b'a' + 10,
                b'A'...b'Z' => b - b'A' + 10,
                b'_' => continue,
                _ => u8::MAX,
            };
            if d < radix as u8 {
                v.push(d);
            } else {
                return Err(ParseBigIntError::invalid());
            }
        }

        let res = if radix.is_power_of_two() {
            // Powers of two can use bitwise masks and shifting instead of multiplication
            let bits = ilog2(radix);
            v.reverse();
            if big_digit::BITS % bits == 0 {
                from_bitwise_digits_le(&v, bits)
            } else {
                from_inexact_bitwise_digits_le(&v, bits)
            }
        } else {
            from_radix_digits_be(&v, radix)
        };
        Ok(res)
    }
}

forward_val_val_binop!(impl BitAnd for BigUint, bitand);
forward_ref_val_binop!(impl BitAnd for BigUint, bitand);

// do not use forward_ref_ref_binop_commutative! for bitand so that we can
// clone the smaller value rather than the larger, avoiding over-allocation
impl<'a, 'b> BitAnd<&'b BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn bitand(self, other: &BigUint) -> BigUint {
        // forward to val-ref, choosing the smaller to clone
        if self.data.len() <= other.data.len() {
            self.clone() & other
        } else {
            other.clone() & self
        }
    }
}

forward_val_assign!(impl BitAndAssign for BigUint, bitand_assign);

impl<'a> BitAnd<&'a BigUint> for BigUint {
    type Output = BigUint;

    #[inline]
    fn bitand(mut self, other: &BigUint) -> BigUint {
        self &= other;
        self
    }
}
impl<'a> BitAndAssign<&'a BigUint> for BigUint {
    #[inline]
    fn bitand_assign(&mut self, other: &BigUint) {
        for (ai, &bi) in self.data.iter_mut().zip(other.data.iter()) {
            *ai &= bi;
        }
        self.data.truncate(other.data.len());
        self.normalize();
    }
}

forward_all_binop_to_val_ref_commutative!(impl BitOr for BigUint, bitor);
forward_val_assign!(impl BitOrAssign for BigUint, bitor_assign);

impl<'a> BitOr<&'a BigUint> for BigUint {
    type Output = BigUint;

    fn bitor(mut self, other: &BigUint) -> BigUint {
        self |= other;
        self
    }
}
impl<'a> BitOrAssign<&'a BigUint> for BigUint {
    #[inline]
    fn bitor_assign(&mut self, other: &BigUint) {
        for (ai, &bi) in self.data.iter_mut().zip(other.data.iter()) {
            *ai |= bi;
        }
        if other.data.len() > self.data.len() {
            let extra = &other.data[self.data.len()..];
            self.data.extend(extra.iter().cloned());
        }
    }
}

forward_all_binop_to_val_ref_commutative!(impl BitXor for BigUint, bitxor);
forward_val_assign!(impl BitXorAssign for BigUint, bitxor_assign);

impl<'a> BitXor<&'a BigUint> for BigUint {
    type Output = BigUint;

    fn bitxor(mut self, other: &BigUint) -> BigUint {
        self ^= other;
        self
    }
}
impl<'a> BitXorAssign<&'a BigUint> for BigUint {
    #[inline]
    fn bitxor_assign(&mut self, other: &BigUint) {
        for (ai, &bi) in self.data.iter_mut().zip(other.data.iter()) {
            *ai ^= bi;
        }
        if other.data.len() > self.data.len() {
            let extra = &other.data[self.data.len()..];
            self.data.extend(extra.iter().cloned());
        }
        self.normalize();
    }
}

impl Shl<usize> for BigUint {
    type Output = BigUint;

    #[inline]
    fn shl(self, rhs: usize) -> BigUint {
        biguint_shl(Cow::Owned(self), rhs)
    }
}
impl<'a> Shl<usize> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn shl(self, rhs: usize) -> BigUint {
        biguint_shl(Cow::Borrowed(self), rhs)
    }
}

impl ShlAssign<usize> for BigUint {
    #[inline]
    fn shl_assign(&mut self, rhs: usize) {
        let n = mem::replace(self, BigUint::zero());
        *self = n << rhs;
    }
}

impl Shr<usize> for BigUint {
    type Output = BigUint;

    #[inline]
    fn shr(self, rhs: usize) -> BigUint {
        biguint_shr(Cow::Owned(self), rhs)
    }
}
impl<'a> Shr<usize> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn shr(self, rhs: usize) -> BigUint {
        biguint_shr(Cow::Borrowed(self), rhs)
    }
}

impl ShrAssign<usize> for BigUint {
    #[inline]
    fn shr_assign(&mut self, rhs: usize) {
        let n = mem::replace(self, BigUint::zero());
        *self = n >> rhs;
    }
}

impl Zero for BigUint {
    #[inline]
    fn zero() -> BigUint {
        BigUint::new(Vec::new())
    }

    #[inline]
    fn set_zero(&mut self) {
        self.data.clear();
    }

    #[inline]
    fn is_zero(&self) -> bool {
        self.data.is_empty()
    }
}

impl One for BigUint {
    #[inline]
    fn one() -> BigUint {
        BigUint::new(vec![1])
    }

    #[inline]
    fn set_one(&mut self) {
        self.data.clear();
        self.data.push(1);
    }

    #[inline]
    fn is_one(&self) -> bool {
        self.data[..] == [1]
    }
}

impl Unsigned for BigUint {}

impl<'a> Pow<BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn pow(self, exp: BigUint) -> Self::Output {
        self.pow(&exp)
    }
}

impl<'a, 'b> Pow<&'b BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn pow(self, exp: &BigUint) -> Self::Output {
        if self.is_one() || exp.is_zero() {
            BigUint::one()
        } else if self.is_zero() {
            BigUint::zero()
        } else if let Some(exp) = exp.to_u64() {
            self.pow(exp)
        } else {
            // At this point, `self >= 2` and `exp >= 2⁶⁴`.  The smallest possible result
            // given `2.pow(2⁶⁴)` would take 2.3 exabytes of memory!
            panic!("memory overflow")
        }
    }
}

macro_rules! pow_impl {
    ($T:ty) => {
        impl<'a> Pow<$T> for &'a BigUint {
            type Output = BigUint;

            #[inline]
            fn pow(self, mut exp: $T) -> Self::Output {
                if exp == 0 {
                    return BigUint::one();
                }
                let mut base = self.clone();

                while exp & 1 == 0 {
                    base = &base * &base;
                    exp >>= 1;
                }

                if exp == 1 {
                    return base;
                }

                let mut acc = base.clone();
                while exp > 1 {
                    exp >>= 1;
                    base = &base * &base;
                    if exp & 1 == 1 {
                        acc = &acc * &base;
                    }
                }
                acc
            }
        }

        impl<'a, 'b> Pow<&'b $T> for &'a BigUint {
            type Output = BigUint;

            #[inline]
            fn pow(self, exp: &$T) -> Self::Output {
                self.pow(*exp)
            }
        }
    };
}

pow_impl!(u8);
pow_impl!(u16);
pow_impl!(u32);
pow_impl!(u64);
pow_impl!(usize);
#[cfg(has_i128)]
pow_impl!(u128);

forward_all_binop_to_val_ref_commutative!(impl Add for BigUint, add);
forward_val_assign!(impl AddAssign for BigUint, add_assign);

impl<'a> Add<&'a BigUint> for BigUint {
    type Output = BigUint;

    fn add(mut self, other: &BigUint) -> BigUint {
        self += other;
        self
    }
}
impl<'a> AddAssign<&'a BigUint> for BigUint {
    #[inline]
    fn add_assign(&mut self, other: &BigUint) {
        let self_len = self.data.len();
        let carry = if self_len < other.data.len() {
            let lo_carry = __add2(&mut self.data[..], &other.data[..self_len]);
            self.data.extend_from_slice(&other.data[self_len..]);
            __add2(&mut self.data[self_len..], &[lo_carry])
        } else {
            __add2(&mut self.data[..], &other.data[..])
        };
        if carry != 0 {
            self.data.push(carry);
        }
    }
}

promote_unsigned_scalars!(impl Add for BigUint, add);
promote_unsigned_scalars_assign!(impl AddAssign for BigUint, add_assign);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u32> for BigUint, add);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u64> for BigUint, add);
#[cfg(has_i128)]
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u128> for BigUint, add);

impl Add<u32> for BigUint {
    type Output = BigUint;

    #[inline]
    fn add(mut self, other: u32) -> BigUint {
        self += other;
        self
    }
}

impl AddAssign<u32> for BigUint {
    #[inline]
    fn add_assign(&mut self, other: u32) {
        if other != 0 {
            if self.data.is_empty() {
                self.data.push(0);
            }

            let carry = __add2(&mut self.data, &[other as BigDigit]);
            if carry != 0 {
                self.data.push(carry);
            }
        }
    }
}

impl Add<u64> for BigUint {
    type Output = BigUint;

    #[inline]
    fn add(mut self, other: u64) -> BigUint {
        self += other;
        self
    }
}

impl AddAssign<u64> for BigUint {
    #[inline]
    fn add_assign(&mut self, other: u64) {
        let (hi, lo) = big_digit::from_doublebigdigit(other);
        if hi == 0 {
            *self += lo;
        } else {
            while self.data.len() < 2 {
                self.data.push(0);
            }

            let carry = __add2(&mut self.data, &[lo, hi]);
            if carry != 0 {
                self.data.push(carry);
            }
        }
    }
}

#[cfg(has_i128)]
impl Add<u128> for BigUint {
    type Output = BigUint;

    #[inline]
    fn add(mut self, other: u128) -> BigUint {
        self += other;
        self
    }
}

#[cfg(has_i128)]
impl AddAssign<u128> for BigUint {
    #[inline]
    fn add_assign(&mut self, other: u128) {
        if other <= u128::from(u64::max_value()) {
            *self += other as u64
        } else {
            let (a, b, c, d) = u32_from_u128(other);
            let carry = if a > 0 {
                while self.data.len() < 4 {
                    self.data.push(0);
                }
                __add2(&mut self.data, &[d, c, b, a])
            } else {
                debug_assert!(b > 0);
                while self.data.len() < 3 {
                    self.data.push(0);
                }
                __add2(&mut self.data, &[d, c, b])
            };

            if carry != 0 {
                self.data.push(carry);
            }
        }
    }
}

forward_val_val_binop!(impl Sub for BigUint, sub);
forward_ref_ref_binop!(impl Sub for BigUint, sub);
forward_val_assign!(impl SubAssign for BigUint, sub_assign);

impl<'a> Sub<&'a BigUint> for BigUint {
    type Output = BigUint;

    fn sub(mut self, other: &BigUint) -> BigUint {
        self -= other;
        self
    }
}
impl<'a> SubAssign<&'a BigUint> for BigUint {
    fn sub_assign(&mut self, other: &'a BigUint) {
        sub2(&mut self.data[..], &other.data[..]);
        self.normalize();
    }
}

impl<'a> Sub<BigUint> for &'a BigUint {
    type Output = BigUint;

    fn sub(self, mut other: BigUint) -> BigUint {
        let other_len = other.data.len();
        if other_len < self.data.len() {
            let lo_borrow = __sub2rev(&self.data[..other_len], &mut other.data);
            other.data.extend_from_slice(&self.data[other_len..]);
            if lo_borrow != 0 {
                sub2(&mut other.data[other_len..], &[1])
            }
        } else {
            sub2rev(&self.data[..], &mut other.data[..]);
        }
        other.normalized()
    }
}

promote_unsigned_scalars!(impl Sub for BigUint, sub);
promote_unsigned_scalars_assign!(impl SubAssign for BigUint, sub_assign);
forward_all_scalar_binop_to_val_val!(impl Sub<u32> for BigUint, sub);
forward_all_scalar_binop_to_val_val!(impl Sub<u64> for BigUint, sub);
#[cfg(has_i128)]
forward_all_scalar_binop_to_val_val!(impl Sub<u128> for BigUint, sub);

impl Sub<u32> for BigUint {
    type Output = BigUint;

    #[inline]
    fn sub(mut self, other: u32) -> BigUint {
        self -= other;
        self
    }
}
impl SubAssign<u32> for BigUint {
    fn sub_assign(&mut self, other: u32) {
        sub2(&mut self.data[..], &[other as BigDigit]);
        self.normalize();
    }
}

impl Sub<BigUint> for u32 {
    type Output = BigUint;

    #[inline]
    fn sub(self, mut other: BigUint) -> BigUint {
        if other.data.is_empty() {
            other.data.push(self as BigDigit);
        } else {
            sub2rev(&[self as BigDigit], &mut other.data[..]);
        }
        other.normalized()
    }
}

impl Sub<u64> for BigUint {
    type Output = BigUint;

    #[inline]
    fn sub(mut self, other: u64) -> BigUint {
        self -= other;
        self
    }
}

impl SubAssign<u64> for BigUint {
    #[inline]
    fn sub_assign(&mut self, other: u64) {
        let (hi, lo) = big_digit::from_doublebigdigit(other);
        sub2(&mut self.data[..], &[lo, hi]);
        self.normalize();
    }
}

impl Sub<BigUint> for u64 {
    type Output = BigUint;

    #[inline]
    fn sub(self, mut other: BigUint) -> BigUint {
        while other.data.len() < 2 {
            other.data.push(0);
        }

        let (hi, lo) = big_digit::from_doublebigdigit(self);
        sub2rev(&[lo, hi], &mut other.data[..]);
        other.normalized()
    }
}

#[cfg(has_i128)]
impl Sub<u128> for BigUint {
    type Output = BigUint;

    #[inline]
    fn sub(mut self, other: u128) -> BigUint {
        self -= other;
        self
    }
}
#[cfg(has_i128)]
impl SubAssign<u128> for BigUint {
    fn sub_assign(&mut self, other: u128) {
        let (a, b, c, d) = u32_from_u128(other);
        sub2(&mut self.data[..], &[d, c, b, a]);
        self.normalize();
    }
}

#[cfg(has_i128)]
impl Sub<BigUint> for u128 {
    type Output = BigUint;

    #[inline]
    fn sub(self, mut other: BigUint) -> BigUint {
        while other.data.len() < 4 {
            other.data.push(0);
        }

        let (a, b, c, d) = u32_from_u128(self);
        sub2rev(&[d, c, b, a], &mut other.data[..]);
        other.normalized()
    }
}

forward_all_binop_to_ref_ref!(impl Mul for BigUint, mul);
forward_val_assign!(impl MulAssign for BigUint, mul_assign);

impl<'a, 'b> Mul<&'b BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn mul(self, other: &BigUint) -> BigUint {
        mul3(&self.data[..], &other.data[..])
    }
}
impl<'a> MulAssign<&'a BigUint> for BigUint {
    #[inline]
    fn mul_assign(&mut self, other: &'a BigUint) {
        *self = &*self * other
    }
}

promote_unsigned_scalars!(impl Mul for BigUint, mul);
promote_unsigned_scalars_assign!(impl MulAssign for BigUint, mul_assign);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u32> for BigUint, mul);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u64> for BigUint, mul);
#[cfg(has_i128)]
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u128> for BigUint, mul);

impl Mul<u32> for BigUint {
    type Output = BigUint;

    #[inline]
    fn mul(mut self, other: u32) -> BigUint {
        self *= other;
        self
    }
}
impl MulAssign<u32> for BigUint {
    #[inline]
    fn mul_assign(&mut self, other: u32) {
        if other == 0 {
            self.data.clear();
        } else {
            let carry = scalar_mul(&mut self.data[..], other as BigDigit);
            if carry != 0 {
                self.data.push(carry);
            }
        }
    }
}

impl Mul<u64> for BigUint {
    type Output = BigUint;

    #[inline]
    fn mul(mut self, other: u64) -> BigUint {
        self *= other;
        self
    }
}
impl MulAssign<u64> for BigUint {
    #[inline]
    fn mul_assign(&mut self, other: u64) {
        if other == 0 {
            self.data.clear();
        } else if other <= u64::from(BigDigit::max_value()) {
            *self *= other as BigDigit
        } else {
            let (hi, lo) = big_digit::from_doublebigdigit(other);
            *self = mul3(&self.data[..], &[lo, hi])
        }
    }
}

#[cfg(has_i128)]
impl Mul<u128> for BigUint {
    type Output = BigUint;

    #[inline]
    fn mul(mut self, other: u128) -> BigUint {
        self *= other;
        self
    }
}
#[cfg(has_i128)]
impl MulAssign<u128> for BigUint {
    #[inline]
    fn mul_assign(&mut self, other: u128) {
        if other == 0 {
            self.data.clear();
        } else if other <= u128::from(BigDigit::max_value()) {
            *self *= other as BigDigit
        } else {
            let (a, b, c, d) = u32_from_u128(other);
            *self = mul3(&self.data[..], &[d, c, b, a])
        }
    }
}

forward_val_ref_binop!(impl Div for BigUint, div);
forward_ref_val_binop!(impl Div for BigUint, div);
forward_val_assign!(impl DivAssign for BigUint, div_assign);

impl Div<BigUint> for BigUint {
    type Output = BigUint;

    #[inline]
    fn div(self, other: BigUint) -> BigUint {
        let (q, _) = div_rem(self, other);
        q
    }
}

impl<'a, 'b> Div<&'b BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn div(self, other: &BigUint) -> BigUint {
        let (q, _) = self.div_rem(other);
        q
    }
}
impl<'a> DivAssign<&'a BigUint> for BigUint {
    #[inline]
    fn div_assign(&mut self, other: &'a BigUint) {
        *self = &*self / other;
    }
}

promote_unsigned_scalars!(impl Div for BigUint, div);
promote_unsigned_scalars_assign!(impl DivAssign for BigUint, div_assign);
forward_all_scalar_binop_to_val_val!(impl Div<u32> for BigUint, div);
forward_all_scalar_binop_to_val_val!(impl Div<u64> for BigUint, div);
#[cfg(has_i128)]
forward_all_scalar_binop_to_val_val!(impl Div<u128> for BigUint, div);

impl Div<u32> for BigUint {
    type Output = BigUint;

    #[inline]
    fn div(self, other: u32) -> BigUint {
        let (q, _) = div_rem_digit(self, other as BigDigit);
        q
    }
}
impl DivAssign<u32> for BigUint {
    #[inline]
    fn div_assign(&mut self, other: u32) {
        *self = &*self / other;
    }
}

impl Div<BigUint> for u32 {
    type Output = BigUint;

    #[inline]
    fn div(self, other: BigUint) -> BigUint {
        match other.data.len() {
            0 => panic!(),
            1 => From::from(self as BigDigit / other.data[0]),
            _ => Zero::zero(),
        }
    }
}

impl Div<u64> for BigUint {
    type Output = BigUint;

    #[inline]
    fn div(self, other: u64) -> BigUint {
        let (q, _) = div_rem(self, From::from(other));
        q
    }
}
impl DivAssign<u64> for BigUint {
    #[inline]
    fn div_assign(&mut self, other: u64) {
        // a vec of size 0 does not allocate, so this is fairly cheap
        let temp = mem::replace(self, Zero::zero());
        *self = temp / other;
    }
}

impl Div<BigUint> for u64 {
    type Output = BigUint;

    #[inline]
    fn div(self, other: BigUint) -> BigUint {
        match other.data.len() {
            0 => panic!(),
            1 => From::from(self / u64::from(other.data[0])),
            2 => From::from(self / big_digit::to_doublebigdigit(other.data[1], other.data[0])),
            _ => Zero::zero(),
        }
    }
}

#[cfg(has_i128)]
impl Div<u128> for BigUint {
    type Output = BigUint;

    #[inline]
    fn div(self, other: u128) -> BigUint {
        let (q, _) = div_rem(self, From::from(other));
        q
    }
}
#[cfg(has_i128)]
impl DivAssign<u128> for BigUint {
    #[inline]
    fn div_assign(&mut self, other: u128) {
        *self = &*self / other;
    }
}

#[cfg(has_i128)]
impl Div<BigUint> for u128 {
    type Output = BigUint;

    #[inline]
    fn div(self, other: BigUint) -> BigUint {
        match other.data.len() {
            0 => panic!(),
            1 => From::from(self / u128::from(other.data[0])),
            2 => From::from(
                self / u128::from(big_digit::to_doublebigdigit(other.data[1], other.data[0])),
            ),
            3 => From::from(self / u32_to_u128(0, other.data[2], other.data[1], other.data[0])),
            4 => From::from(
                self / u32_to_u128(other.data[3], other.data[2], other.data[1], other.data[0]),
            ),
            _ => Zero::zero(),
        }
    }
}

forward_val_ref_binop!(impl Rem for BigUint, rem);
forward_ref_val_binop!(impl Rem for BigUint, rem);
forward_val_assign!(impl RemAssign for BigUint, rem_assign);

impl Rem<BigUint> for BigUint {
    type Output = BigUint;

    #[inline]
    fn rem(self, other: BigUint) -> BigUint {
        let (_, r) = div_rem(self, other);
        r
    }
}

impl<'a, 'b> Rem<&'b BigUint> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn rem(self, other: &BigUint) -> BigUint {
        let (_, r) = self.div_rem(other);
        r
    }
}
impl<'a> RemAssign<&'a BigUint> for BigUint {
    #[inline]
    fn rem_assign(&mut self, other: &BigUint) {
        *self = &*self % other;
    }
}

promote_unsigned_scalars!(impl Rem for BigUint, rem);
promote_unsigned_scalars_assign!(impl RemAssign for BigUint, rem_assign);
forward_all_scalar_binop_to_ref_val!(impl Rem<u32> for BigUint, rem);
forward_all_scalar_binop_to_val_val!(impl Rem<u64> for BigUint, rem);
#[cfg(has_i128)]
forward_all_scalar_binop_to_val_val!(impl Rem<u128> for BigUint, rem);

impl<'a> Rem<u32> for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn rem(self, other: u32) -> BigUint {
        From::from(rem_digit(self, other as BigDigit))
    }
}
impl RemAssign<u32> for BigUint {
    #[inline]
    fn rem_assign(&mut self, other: u32) {
        *self = &*self % other;
    }
}

impl<'a> Rem<&'a BigUint> for u32 {
    type Output = BigUint;

    #[inline]
    fn rem(mut self, other: &'a BigUint) -> BigUint {
        self %= other;
        From::from(self)
    }
}

macro_rules! impl_rem_assign_scalar {
    ($scalar:ty, $to_scalar:ident) => {
        forward_val_assign_scalar!(impl RemAssign for BigUint, $scalar, rem_assign);
        impl<'a> RemAssign<&'a BigUint> for $scalar {
            #[inline]
            fn rem_assign(&mut self, other: &BigUint) {
                *self = match other.$to_scalar() {
                    None => *self,
                    Some(0) => panic!(),
                    Some(v) => *self % v
                };
            }
        }
    }
}
// we can scalar %= BigUint for any scalar, including signed types
#[cfg(has_i128)]
impl_rem_assign_scalar!(u128, to_u128);
impl_rem_assign_scalar!(usize, to_usize);
impl_rem_assign_scalar!(u64, to_u64);
impl_rem_assign_scalar!(u32, to_u32);
impl_rem_assign_scalar!(u16, to_u16);
impl_rem_assign_scalar!(u8, to_u8);
#[cfg(has_i128)]
impl_rem_assign_scalar!(i128, to_i128);
impl_rem_assign_scalar!(isize, to_isize);
impl_rem_assign_scalar!(i64, to_i64);
impl_rem_assign_scalar!(i32, to_i32);
impl_rem_assign_scalar!(i16, to_i16);
impl_rem_assign_scalar!(i8, to_i8);

impl Rem<u64> for BigUint {
    type Output = BigUint;

    #[inline]
    fn rem(self, other: u64) -> BigUint {
        let (_, r) = div_rem(self, From::from(other));
        r
    }
}
impl RemAssign<u64> for BigUint {
    #[inline]
    fn rem_assign(&mut self, other: u64) {
        *self = &*self % other;
    }
}

impl Rem<BigUint> for u64 {
    type Output = BigUint;

    #[inline]
    fn rem(mut self, other: BigUint) -> BigUint {
        self %= other;
        From::from(self)
    }
}

#[cfg(has_i128)]
impl Rem<u128> for BigUint {
    type Output = BigUint;

    #[inline]
    fn rem(self, other: u128) -> BigUint {
        let (_, r) = div_rem(self, From::from(other));
        r
    }
}
#[cfg(has_i128)]
impl RemAssign<u128> for BigUint {
    #[inline]
    fn rem_assign(&mut self, other: u128) {
        *self = &*self % other;
    }
}

#[cfg(has_i128)]
impl Rem<BigUint> for u128 {
    type Output = BigUint;

    #[inline]
    fn rem(mut self, other: BigUint) -> BigUint {
        self %= other;
        From::from(self)
    }
}

impl Neg for BigUint {
    type Output = BigUint;

    #[inline]
    fn neg(self) -> BigUint {
        panic!()
    }
}

impl<'a> Neg for &'a BigUint {
    type Output = BigUint;

    #[inline]
    fn neg(self) -> BigUint {
        panic!()
    }
}

impl CheckedAdd for BigUint {
    #[inline]
    fn checked_add(&self, v: &BigUint) -> Option<BigUint> {
        Some(self.add(v))
    }
}

impl CheckedSub for BigUint {
    #[inline]
    fn checked_sub(&self, v: &BigUint) -> Option<BigUint> {
        match self.cmp(v) {
            Less => None,
            Equal => Some(Zero::zero()),
            Greater => Some(self.sub(v)),
        }
    }
}

impl CheckedMul for BigUint {
    #[inline]
    fn checked_mul(&self, v: &BigUint) -> Option<BigUint> {
        Some(self.mul(v))
    }
}

impl CheckedDiv for BigUint {
    #[inline]
    fn checked_div(&self, v: &BigUint) -> Option<BigUint> {
        if v.is_zero() {
            return None;
        }
        Some(self.div(v))
    }
}

impl Integer for BigUint {
    #[inline]
    fn div_rem(&self, other: &BigUint) -> (BigUint, BigUint) {
        div_rem_ref(self, other)
    }

    #[inline]
    fn div_floor(&self, other: &BigUint) -> BigUint {
        let (d, _) = div_rem_ref(self, other);
        d
    }

    #[inline]
    fn mod_floor(&self, other: &BigUint) -> BigUint {
        let (_, m) = div_rem_ref(self, other);
        m
    }

    #[inline]
    fn div_mod_floor(&self, other: &BigUint) -> (BigUint, BigUint) {
        div_rem_ref(self, other)
    }

    /// Calculates the Greatest Common Divisor (GCD) of the number and `other`.
    ///
    /// The result is always positive.
    #[inline]
    fn gcd(&self, other: &Self) -> Self {
        #[inline]
        fn twos(x: &BigUint) -> usize {
            trailing_zeros(x).unwrap_or(0)
        }

        // Stein's algorithm
        if self.is_zero() {
            return other.clone();
        }
        if other.is_zero() {
            return self.clone();
        }
        let mut m = self.clone();
        let mut n = other.clone();

        // find common factors of 2
        let shift = cmp::min(twos(&n), twos(&m));

        // divide m and n by 2 until odd
        // m inside loop
        n >>= twos(&n);

        while !m.is_zero() {
            m >>= twos(&m);
            if n > m {
                mem::swap(&mut n, &mut m)
            }
            m -= &n;
        }

        n << shift
    }

    /// Calculates the Lowest Common Multiple (LCM) of the number and `other`.
    #[inline]
    fn lcm(&self, other: &BigUint) -> BigUint {
        if self.is_zero() && other.is_zero() {
            Self::zero()
        } else {
            self / self.gcd(other) * other
        }
    }

    /// Deprecated, use `is_multiple_of` instead.
    #[inline]
    fn divides(&self, other: &BigUint) -> bool {
        self.is_multiple_of(other)
    }

    /// Returns `true` if the number is a multiple of `other`.
    #[inline]
    fn is_multiple_of(&self, other: &BigUint) -> bool {
        (self % other).is_zero()
    }

    /// Returns `true` if the number is divisible by `2`.
    #[inline]
    fn is_even(&self) -> bool {
        // Considering only the last digit.
        match self.data.first() {
            Some(x) => x.is_even(),
            None => true,
        }
    }

    /// Returns `true` if the number is not divisible by `2`.
    #[inline]
    fn is_odd(&self) -> bool {
        !self.is_even()
    }
}

#[inline]
fn fixpoint<F>(mut x: BigUint, max_bits: usize, f: F) -> BigUint
where
    F: Fn(&BigUint) -> BigUint,
{
    let mut xn = f(&x);

    // If the value increased, then the initial guess must have been low.
    // Repeat until we reverse course.
    while x < xn {
        // Sometimes an increase will go way too far, especially with large
        // powers, and then take a long time to walk back.  We know an upper
        // bound based on bit size, so saturate on that.
        x = if xn.bits() > max_bits {
            BigUint::one() << max_bits
        } else {
            xn
        };
        xn = f(&x);
    }

    // Now keep repeating while the estimate is decreasing.
    while x > xn {
        x = xn;
        xn = f(&x);
    }
    x
}

impl Roots for BigUint {
    // nth_root, sqrt and cbrt use Newton's method to compute
    // principal root of a given degree for a given integer.

    // Reference:
    // Brent & Zimmermann, Modern Computer Arithmetic, v0.5.9, Algorithm 1.14
    fn nth_root(&self, n: u32) -> Self {
        assert!(n > 0, "root degree n must be at least 1");

        if self.is_zero() || self.is_one() {
            return self.clone();
        }

        match n {
            // Optimize for small n
            1 => return self.clone(),
            2 => return self.sqrt(),
            3 => return self.cbrt(),
            _ => (),
        }

        // The root of non-zero values less than 2ⁿ can only be 1.
        let bits = self.bits();
        if bits <= n as usize {
            return BigUint::one();
        }

        // If we fit in `u64`, compute the root that way.
        if let Some(x) = self.to_u64() {
            return x.nth_root(n).into();
        }

        let max_bits = bits / n as usize + 1;

        let guess = if let Some(f) = self.to_f64() {
            // We fit in `f64` (lossy), so get a better initial guess from that.
            BigUint::from_f64((f.ln() / f64::from(n)).exp()).unwrap()
        } else {
            // Try to guess by scaling down such that it does fit in `f64`.
            // With some (x * 2ⁿᵏ), its nth root ≈ (ⁿ√x * 2ᵏ)
            let nsz = n as usize;
            let extra_bits = bits - (f64::MAX_EXP as usize - 1);
            let root_scale = (extra_bits + (nsz - 1)) / nsz;
            let scale = root_scale * nsz;
            if scale < bits && bits - scale > nsz {
                (self >> scale).nth_root(n) << root_scale
            } else {
                BigUint::one() << max_bits
            }
        };

        let n_min_1 = n - 1;
        fixpoint(guess, max_bits, move |s| {
            let q = self / s.pow(n_min_1);
            let t = n_min_1 * s + q;
            t / n
        })
    }

    // Reference:
    // Brent & Zimmermann, Modern Computer Arithmetic, v0.5.9, Algorithm 1.13
    fn sqrt(&self) -> Self {
        if self.is_zero() || self.is_one() {
            return self.clone();
        }

        // If we fit in `u64`, compute the root that way.
        if let Some(x) = self.to_u64() {
            return x.sqrt().into();
        }

        let bits = self.bits();
        let max_bits = bits / 2 as usize + 1;

        let guess = if let Some(f) = self.to_f64() {
            // We fit in `f64` (lossy), so get a better initial guess from that.
            BigUint::from_f64(f.sqrt()).unwrap()
        } else {
            // Try to guess by scaling down such that it does fit in `f64`.
            // With some (x * 2²ᵏ), its sqrt ≈ (√x * 2ᵏ)
            let extra_bits = bits - (f64::MAX_EXP as usize - 1);
            let root_scale = (extra_bits + 1) / 2;
            let scale = root_scale * 2;
            (self >> scale).sqrt() << root_scale
        };

        fixpoint(guess, max_bits, move |s| {
            let q = self / s;
            let t = s + q;
            t >> 1
        })
    }

    fn cbrt(&self) -> Self {
        if self.is_zero() || self.is_one() {
            return self.clone();
        }

        // If we fit in `u64`, compute the root that way.
        if let Some(x) = self.to_u64() {
            return x.cbrt().into();
        }

        let bits = self.bits();
        let max_bits = bits / 3 as usize + 1;

        let guess = if let Some(f) = self.to_f64() {
            // We fit in `f64` (lossy), so get a better initial guess from that.
            BigUint::from_f64(f.cbrt()).unwrap()
        } else {
            // Try to guess by scaling down such that it does fit in `f64`.
            // With some (x * 2³ᵏ), its cbrt ≈ (∛x * 2ᵏ)
            let extra_bits = bits - (f64::MAX_EXP as usize - 1);
            let root_scale = (extra_bits + 2) / 3;
            let scale = root_scale * 3;
            (self >> scale).cbrt() << root_scale
        };

        fixpoint(guess, max_bits, move |s| {
            let q = self / (s * s);
            let t = (s << 1) + q;
            t / 3u32
        })
    }
}

fn high_bits_to_u64(v: &BigUint) -> u64 {
    match v.data.len() {
        0 => 0,
        1 => u64::from(v.data[0]),
        _ => {
            let mut bits = v.bits();
            let mut ret = 0u64;
            let mut ret_bits = 0;

            for d in v.data.iter().rev() {
                let digit_bits = (bits - 1) % big_digit::BITS + 1;
                let bits_want = cmp::min(64 - ret_bits, digit_bits);

                if bits_want != 64 {
                    ret <<= bits_want;
                }
                ret |= u64::from(*d) >> (digit_bits - bits_want);
                ret_bits += bits_want;
                bits -= bits_want;

                if ret_bits == 64 {
                    break;
                }
            }

            ret
        }
    }
}

impl ToPrimitive for BigUint {
    #[inline]
    fn to_i64(&self) -> Option<i64> {
        self.to_u64().as_ref().and_then(u64::to_i64)
    }

    #[inline]
    #[cfg(has_i128)]
    fn to_i128(&self) -> Option<i128> {
        self.to_u128().as_ref().and_then(u128::to_i128)
    }

    #[inline]
    fn to_u64(&self) -> Option<u64> {
        let mut ret: u64 = 0;
        let mut bits = 0;

        for i in self.data.iter() {
            if bits >= 64 {
                return None;
            }

            ret += u64::from(*i) << bits;
            bits += big_digit::BITS;
        }

        Some(ret)
    }

    #[inline]
    #[cfg(has_i128)]
    fn to_u128(&self) -> Option<u128> {
        let mut ret: u128 = 0;
        let mut bits = 0;

        for i in self.data.iter() {
            if bits >= 128 {
                return None;
            }

            ret |= u128::from(*i) << bits;
            bits += big_digit::BITS;
        }

        Some(ret)
    }

    #[inline]
    fn to_f32(&self) -> Option<f32> {
        let mantissa = high_bits_to_u64(self);
        let exponent = self.bits() - fls(mantissa);

        if exponent > f32::MAX_EXP as usize {
            None
        } else {
            let ret = (mantissa as f32) * 2.0f32.powi(exponent as i32);
            if ret.is_infinite() {
                None
            } else {
                Some(ret)
            }
        }
    }

    #[inline]
    fn to_f64(&self) -> Option<f64> {
        let mantissa = high_bits_to_u64(self);
        let exponent = self.bits() - fls(mantissa);

        if exponent > f64::MAX_EXP as usize {
            None
        } else {
            let ret = (mantissa as f64) * 2.0f64.powi(exponent as i32);
            if ret.is_infinite() {
                None
            } else {
                Some(ret)
            }
        }
    }
}

impl FromPrimitive for BigUint {
    #[inline]
    fn from_i64(n: i64) -> Option<BigUint> {
        if n >= 0 {
            Some(BigUint::from(n as u64))
        } else {
            None
        }
    }

    #[inline]
    #[cfg(has_i128)]
    fn from_i128(n: i128) -> Option<BigUint> {
        if n >= 0 {
            Some(BigUint::from(n as u128))
        } else {
            None
        }
    }

    #[inline]
    fn from_u64(n: u64) -> Option<BigUint> {
        Some(BigUint::from(n))
    }

    #[inline]
    #[cfg(has_i128)]
    fn from_u128(n: u128) -> Option<BigUint> {
        Some(BigUint::from(n))
    }

    #[inline]
    fn from_f64(mut n: f64) -> Option<BigUint> {
        // handle NAN, INFINITY, NEG_INFINITY
        if !n.is_finite() {
            return None;
        }

        // match the rounding of casting from float to int
        n = n.trunc();

        // handle 0.x, -0.x
        if n.is_zero() {
            return Some(BigUint::zero());
        }

        let (mantissa, exponent, sign) = Float::integer_decode(n);

        if sign == -1 {
            return None;
        }

        let mut ret = BigUint::from(mantissa);
        if exponent > 0 {
            ret <<= exponent as usize;
        } else if exponent < 0 {
            ret >>= (-exponent) as usize;
        }
        Some(ret)
    }
}

impl From<u64> for BigUint {
    #[inline]
    fn from(mut n: u64) -> Self {
        let mut ret: BigUint = Zero::zero();

        while n != 0 {
            ret.data.push(n as BigDigit);
            // don't overflow if BITS is 64:
            n = (n >> 1) >> (big_digit::BITS - 1);
        }

        ret
    }
}

#[cfg(has_i128)]
impl From<u128> for BigUint {
    #[inline]
    fn from(mut n: u128) -> Self {
        let mut ret: BigUint = Zero::zero();

        while n != 0 {
            ret.data.push(n as BigDigit);
            n >>= big_digit::BITS;
        }

        ret
    }
}

macro_rules! impl_biguint_from_uint {
    ($T:ty) => {
        impl From<$T> for BigUint {
            #[inline]
            fn from(n: $T) -> Self {
                BigUint::from(n as u64)
            }
        }
    };
}

impl_biguint_from_uint!(u8);
impl_biguint_from_uint!(u16);
impl_biguint_from_uint!(u32);
impl_biguint_from_uint!(usize);

/// A generic trait for converting a value to a `BigUint`.
pub trait ToBigUint {
    /// Converts the value of `self` to a `BigUint`.
    fn to_biguint(&self) -> Option<BigUint>;
}

impl ToBigUint for BigUint {
    #[inline]
    fn to_biguint(&self) -> Option<BigUint> {
        Some(self.clone())
    }
}

macro_rules! impl_to_biguint {
    ($T:ty, $from_ty:path) => {
        impl ToBigUint for $T {
            #[inline]
            fn to_biguint(&self) -> Option<BigUint> {
                $from_ty(*self)
            }
        }
    };
}

impl_to_biguint!(isize, FromPrimitive::from_isize);
impl_to_biguint!(i8, FromPrimitive::from_i8);
impl_to_biguint!(i16, FromPrimitive::from_i16);
impl_to_biguint!(i32, FromPrimitive::from_i32);
impl_to_biguint!(i64, FromPrimitive::from_i64);
#[cfg(has_i128)]
impl_to_biguint!(i128, FromPrimitive::from_i128);

impl_to_biguint!(usize, FromPrimitive::from_usize);
impl_to_biguint!(u8, FromPrimitive::from_u8);
impl_to_biguint!(u16, FromPrimitive::from_u16);
impl_to_biguint!(u32, FromPrimitive::from_u32);
impl_to_biguint!(u64, FromPrimitive::from_u64);
#[cfg(has_i128)]
impl_to_biguint!(u128, FromPrimitive::from_u128);

impl_to_biguint!(f32, FromPrimitive::from_f32);
impl_to_biguint!(f64, FromPrimitive::from_f64);

// Extract bitwise digits that evenly divide BigDigit
fn to_bitwise_digits_le(u: &BigUint, bits: usize) -> Vec<u8> {
    debug_assert!(!u.is_zero() && bits <= 8 && big_digit::BITS % bits == 0);

    let last_i = u.data.len() - 1;
    let mask: BigDigit = (1 << bits) - 1;
    let digits_per_big_digit = big_digit::BITS / bits;
    let digits = (u.bits() + bits - 1) / bits;
    let mut res = Vec::with_capacity(digits);

    for mut r in u.data[..last_i].iter().cloned() {
        for _ in 0..digits_per_big_digit {
            res.push((r & mask) as u8);
            r >>= bits;
        }
    }

    let mut r = u.data[last_i];
    while r != 0 {
        res.push((r & mask) as u8);
        r >>= bits;
    }

    res
}

// Extract bitwise digits that don't evenly divide BigDigit
fn to_inexact_bitwise_digits_le(u: &BigUint, bits: usize) -> Vec<u8> {
    debug_assert!(!u.is_zero() && bits <= 8 && big_digit::BITS % bits != 0);

    let mask: BigDigit = (1 << bits) - 1;
    let digits = (u.bits() + bits - 1) / bits;
    let mut res = Vec::with_capacity(digits);

    let mut r = 0;
    let mut rbits = 0;

    for c in &u.data {
        r |= *c << rbits;
        rbits += big_digit::BITS;

        while rbits >= bits {
            res.push((r & mask) as u8);
            r >>= bits;

            // r had more bits than it could fit - grab the bits we lost
            if rbits > big_digit::BITS {
                r = *c >> (big_digit::BITS - (rbits - bits));
            }

            rbits -= bits;
        }
    }

    if rbits != 0 {
        res.push(r as u8);
    }

    while let Some(&0) = res.last() {
        res.pop();
    }

    res
}

// Extract little-endian radix digits
#[inline(always)] // forced inline to get const-prop for radix=10
fn to_radix_digits_le(u: &BigUint, radix: u32) -> Vec<u8> {
    debug_assert!(!u.is_zero() && !radix.is_power_of_two());

    // Estimate how big the result will be, so we can pre-allocate it.
    let radix_digits = ((u.bits() as f64) / f64::from(radix).log2()).ceil();
    let mut res = Vec::with_capacity(radix_digits as usize);
    let mut digits = u.clone();

    let (base, power) = get_radix_base(radix);
    let radix = radix as BigDigit;

    while digits.data.len() > 1 {
        let (q, mut r) = div_rem_digit(digits, base);
        for _ in 0..power {
            res.push((r % radix) as u8);
            r /= radix;
        }
        digits = q;
    }

    let mut r = digits.data[0];
    while r != 0 {
        res.push((r % radix) as u8);
        r /= radix;
    }

    res
}

pub fn to_radix_le(u: &BigUint, radix: u32) -> Vec<u8> {
    if u.is_zero() {
        vec![0]
    } else if radix.is_power_of_two() {
        // Powers of two can use bitwise masks and shifting instead of division
        let bits = ilog2(radix);
        if big_digit::BITS % bits == 0 {
            to_bitwise_digits_le(u, bits)
        } else {
            to_inexact_bitwise_digits_le(u, bits)
        }
    } else if radix == 10 {
        // 10 is so common that it's worth separating out for const-propagation.
        // Optimizers can often turn constant division into a faster multiplication.
        to_radix_digits_le(u, 10)
    } else {
        to_radix_digits_le(u, radix)
    }
}

pub fn to_str_radix_reversed(u: &BigUint, radix: u32) -> Vec<u8> {
    assert!(2 <= radix && radix <= 36, "The radix must be within 2...36");

    if u.is_zero() {
        return vec![b'0'];
    }

    let mut res = to_radix_le(u, radix);

    // Now convert everything to ASCII digits.
    for r in &mut res {
        debug_assert!(u32::from(*r) < radix);
        if *r < 10 {
            *r += b'0';
        } else {
            *r += b'a' - 10;
        }
    }
    res
}

impl BigUint {
    /// Creates and initializes a `BigUint`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn new(digits: Vec<u32>) -> BigUint {
        BigUint { data: digits }.normalized()
    }

    /// Creates and initializes a `BigUint`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn from_slice(slice: &[u32]) -> BigUint {
        BigUint::new(slice.to_vec())
    }

    /// Assign a value to a `BigUint`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn assign_from_slice(&mut self, slice: &[u32]) {
        self.data.resize(slice.len(), 0);
        self.data.clone_from_slice(slice);
        self.normalize();
    }

    /// Creates and initializes a `BigUint`.
    ///
    /// The bytes are in big-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// assert_eq!(BigUint::from_bytes_be(b"A"),
    ///            BigUint::parse_bytes(b"65", 10).unwrap());
    /// assert_eq!(BigUint::from_bytes_be(b"AA"),
    ///            BigUint::parse_bytes(b"16705", 10).unwrap());
    /// assert_eq!(BigUint::from_bytes_be(b"AB"),
    ///            BigUint::parse_bytes(b"16706", 10).unwrap());
    /// assert_eq!(BigUint::from_bytes_be(b"Hello world!"),
    ///            BigUint::parse_bytes(b"22405534230753963835153736737", 10).unwrap());
    /// ```
    #[inline]
    pub fn from_bytes_be(bytes: &[u8]) -> BigUint {
        if bytes.is_empty() {
            Zero::zero()
        } else {
            let mut v = bytes.to_vec();
            v.reverse();
            BigUint::from_bytes_le(&*v)
        }
    }

    /// Creates and initializes a `BigUint`.
    ///
    /// The bytes are in little-endian byte order.
    #[inline]
    pub fn from_bytes_le(bytes: &[u8]) -> BigUint {
        if bytes.is_empty() {
            Zero::zero()
        } else {
            from_bitwise_digits_le(bytes, 8)
        }
    }

    /// Creates and initializes a `BigUint`. The input slice must contain
    /// ascii/utf8 characters in [0-9a-zA-Z].
    /// `radix` must be in the range `2...36`.
    ///
    /// The function `from_str_radix` from the `Num` trait provides the same logic
    /// for `&str` buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigUint, ToBigUint};
    ///
    /// assert_eq!(BigUint::parse_bytes(b"1234", 10), ToBigUint::to_biguint(&1234));
    /// assert_eq!(BigUint::parse_bytes(b"ABCD", 16), ToBigUint::to_biguint(&0xABCD));
    /// assert_eq!(BigUint::parse_bytes(b"G", 16), None);
    /// ```
    #[inline]
    pub fn parse_bytes(buf: &[u8], radix: u32) -> Option<BigUint> {
        str::from_utf8(buf)
            .ok()
            .and_then(|s| BigUint::from_str_radix(s, radix).ok())
    }

    /// Creates and initializes a `BigUint`. Each u8 of the input slice is
    /// interpreted as one digit of the number
    /// and must therefore be less than `radix`.
    ///
    /// The bytes are in big-endian byte order.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigUint};
    ///
    /// let inbase190 = &[15, 33, 125, 12, 14];
    /// let a = BigUint::from_radix_be(inbase190, 190).unwrap();
    /// assert_eq!(a.to_radix_be(190), inbase190);
    /// ```
    pub fn from_radix_be(buf: &[u8], radix: u32) -> Option<BigUint> {
        assert!(
            2 <= radix && radix <= 256,
            "The radix must be within 2...256"
        );

        if radix != 256 && buf.iter().any(|&b| b >= radix as u8) {
            return None;
        }

        let res = if radix.is_power_of_two() {
            // Powers of two can use bitwise masks and shifting instead of multiplication
            let bits = ilog2(radix);
            let mut v = Vec::from(buf);
            v.reverse();
            if big_digit::BITS % bits == 0 {
                from_bitwise_digits_le(&v, bits)
            } else {
                from_inexact_bitwise_digits_le(&v, bits)
            }
        } else {
            from_radix_digits_be(buf, radix)
        };

        Some(res)
    }

    /// Creates and initializes a `BigUint`. Each u8 of the input slice is
    /// interpreted as one digit of the number
    /// and must therefore be less than `radix`.
    ///
    /// The bytes are in little-endian byte order.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigUint};
    ///
    /// let inbase190 = &[14, 12, 125, 33, 15];
    /// let a = BigUint::from_radix_be(inbase190, 190).unwrap();
    /// assert_eq!(a.to_radix_be(190), inbase190);
    /// ```
    pub fn from_radix_le(buf: &[u8], radix: u32) -> Option<BigUint> {
        assert!(
            2 <= radix && radix <= 256,
            "The radix must be within 2...256"
        );

        if radix != 256 && buf.iter().any(|&b| b >= radix as u8) {
            return None;
        }

        let res = if radix.is_power_of_two() {
            // Powers of two can use bitwise masks and shifting instead of multiplication
            let bits = ilog2(radix);
            if big_digit::BITS % bits == 0 {
                from_bitwise_digits_le(buf, bits)
            } else {
                from_inexact_bitwise_digits_le(buf, bits)
            }
        } else {
            let mut v = Vec::from(buf);
            v.reverse();
            from_radix_digits_be(&v, radix)
        };

        Some(res)
    }

    /// Returns the byte representation of the `BigUint` in big-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// let i = BigUint::parse_bytes(b"1125", 10).unwrap();
    /// assert_eq!(i.to_bytes_be(), vec![4, 101]);
    /// ```
    #[inline]
    pub fn to_bytes_be(&self) -> Vec<u8> {
        let mut v = self.to_bytes_le();
        v.reverse();
        v
    }

    /// Returns the byte representation of the `BigUint` in little-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// let i = BigUint::parse_bytes(b"1125", 10).unwrap();
    /// assert_eq!(i.to_bytes_le(), vec![101, 4]);
    /// ```
    #[inline]
    pub fn to_bytes_le(&self) -> Vec<u8> {
        if self.is_zero() {
            vec![0]
        } else {
            to_bitwise_digits_le(self, 8)
        }
    }

    /// Returns the `u32` digits representation of the `BigUint` ordered least significant digit
    /// first.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// assert_eq!(BigUint::from(1125u32).to_u32_digits(), vec![1125]);
    /// assert_eq!(BigUint::from(4294967295u32).to_u32_digits(), vec![4294967295]);
    /// assert_eq!(BigUint::from(4294967296u64).to_u32_digits(), vec![0, 1]);
    /// assert_eq!(BigUint::from(112500000000u64).to_u32_digits(), vec![830850304, 26]);
    /// ```
    #[inline]
    pub fn to_u32_digits(&self) -> Vec<u32> {
        self.data.clone()
    }

    /// Returns the integer formatted as a string in the given radix.
    /// `radix` must be in the range `2...36`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// let i = BigUint::parse_bytes(b"ff", 16).unwrap();
    /// assert_eq!(i.to_str_radix(16), "ff");
    /// ```
    #[inline]
    pub fn to_str_radix(&self, radix: u32) -> String {
        let mut v = to_str_radix_reversed(self, radix);
        v.reverse();
        unsafe { String::from_utf8_unchecked(v) }
    }

    /// Returns the integer in the requested base in big-endian digit order.
    /// The output is not given in a human readable alphabet but as a zero
    /// based u8 number.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// assert_eq!(BigUint::from(0xFFFFu64).to_radix_be(159),
    ///            vec![2, 94, 27]);
    /// // 0xFFFF = 65535 = 2*(159^2) + 94*159 + 27
    /// ```
    #[inline]
    pub fn to_radix_be(&self, radix: u32) -> Vec<u8> {
        let mut v = to_radix_le(self, radix);
        v.reverse();
        v
    }

    /// Returns the integer in the requested base in little-endian digit order.
    /// The output is not given in a human readable alphabet but as a zero
    /// based u8 number.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigUint;
    ///
    /// assert_eq!(BigUint::from(0xFFFFu64).to_radix_le(159),
    ///            vec![27, 94, 2]);
    /// // 0xFFFF = 65535 = 27 + 94*159 + 2*(159^2)
    /// ```
    #[inline]
    pub fn to_radix_le(&self, radix: u32) -> Vec<u8> {
        to_radix_le(self, radix)
    }

    /// Determines the fewest bits necessary to express the `BigUint`.
    #[inline]
    pub fn bits(&self) -> usize {
        if self.is_zero() {
            return 0;
        }
        let zeros = self.data.last().unwrap().leading_zeros();
        self.data.len() * big_digit::BITS - zeros as usize
    }

    /// Strips off trailing zero bigdigits - comparisons require the last element in the vector to
    /// be nonzero.
    #[inline]
    fn normalize(&mut self) {
        while let Some(&0) = self.data.last() {
            self.data.pop();
        }
    }

    /// Returns a normalized `BigUint`.
    #[inline]
    fn normalized(mut self) -> BigUint {
        self.normalize();
        self
    }

    /// Returns `(self ^ exponent) % modulus`.
    ///
    /// Panics if the modulus is zero.
    pub fn modpow(&self, exponent: &Self, modulus: &Self) -> Self {
        assert!(!modulus.is_zero(), "divide by zero!");

        if modulus.is_odd() {
            // For an odd modulus, we can use Montgomery multiplication in base 2^32.
            monty_modpow(self, exponent, modulus)
        } else {
            // Otherwise do basically the same as `num::pow`, but with a modulus.
            plain_modpow(self, &exponent.data, modulus)
        }
    }

    /// Returns the truncated principal square root of `self` --
    /// see [Roots::sqrt](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#method.sqrt)
    pub fn sqrt(&self) -> Self {
        Roots::sqrt(self)
    }

    /// Returns the truncated principal cube root of `self` --
    /// see [Roots::cbrt](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#method.cbrt).
    pub fn cbrt(&self) -> Self {
        Roots::cbrt(self)
    }

    /// Returns the truncated principal `n`th root of `self` --
    /// see [Roots::nth_root](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#tymethod.nth_root).
    pub fn nth_root(&self, n: u32) -> Self {
        Roots::nth_root(self, n)
    }
}

fn plain_modpow(base: &BigUint, exp_data: &[BigDigit], modulus: &BigUint) -> BigUint {
    assert!(!modulus.is_zero(), "divide by zero!");

    let i = match exp_data.iter().position(|&r| r != 0) {
        None => return BigUint::one(),
        Some(i) => i,
    };

    let mut base = base % modulus;
    for _ in 0..i {
        for _ in 0..big_digit::BITS {
            base = &base * &base % modulus;
        }
    }

    let mut r = exp_data[i];
    let mut b = 0usize;
    while r.is_even() {
        base = &base * &base % modulus;
        r >>= 1;
        b += 1;
    }

    let mut exp_iter = exp_data[i + 1..].iter();
    if exp_iter.len() == 0 && r.is_one() {
        return base;
    }

    let mut acc = base.clone();
    r >>= 1;
    b += 1;

    {
        let mut unit = |exp_is_odd| {
            base = &base * &base % modulus;
            if exp_is_odd {
                acc = &acc * &base % modulus;
            }
        };

        if let Some(&last) = exp_iter.next_back() {
            // consume exp_data[i]
            for _ in b..big_digit::BITS {
                unit(r.is_odd());
                r >>= 1;
            }

            // consume all other digits before the last
            for &r in exp_iter {
                let mut r = r;
                for _ in 0..big_digit::BITS {
                    unit(r.is_odd());
                    r >>= 1;
                }
            }
            r = last;
        }

        debug_assert_ne!(r, 0);
        while !r.is_zero() {
            unit(r.is_odd());
            r >>= 1;
        }
    }
    acc
}

#[test]
fn test_plain_modpow() {
    let two = BigUint::from(2u32);
    let modulus = BigUint::from(0x1100u32);

    let exp = vec![0, 0b1];
    assert_eq!(
        two.pow(0b1_00000000_u32) % &modulus,
        plain_modpow(&two, &exp, &modulus)
    );
    let exp = vec![0, 0b10];
    assert_eq!(
        two.pow(0b10_00000000_u32) % &modulus,
        plain_modpow(&two, &exp, &modulus)
    );
    let exp = vec![0, 0b110010];
    assert_eq!(
        two.pow(0b110010_00000000_u32) % &modulus,
        plain_modpow(&two, &exp, &modulus)
    );
    let exp = vec![0b1, 0b1];
    assert_eq!(
        two.pow(0b1_00000001_u32) % &modulus,
        plain_modpow(&two, &exp, &modulus)
    );
    let exp = vec![0b1100, 0, 0b1];
    assert_eq!(
        two.pow(0b1_00000000_00001100_u32) % &modulus,
        plain_modpow(&two, &exp, &modulus)
    );
}

/// Returns the number of least-significant bits that are zero,
/// or `None` if the entire number is zero.
pub fn trailing_zeros(u: &BigUint) -> Option<usize> {
    u.data
        .iter()
        .enumerate()
        .find(|&(_, &digit)| digit != 0)
        .map(|(i, digit)| i * big_digit::BITS + digit.trailing_zeros() as usize)
}

impl_sum_iter_type!(BigUint);
impl_product_iter_type!(BigUint);

pub trait IntDigits {
    fn digits(&self) -> &[BigDigit];
    fn digits_mut(&mut self) -> &mut Vec<BigDigit>;
    fn normalize(&mut self);
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
}

impl IntDigits for BigUint {
    #[inline]
    fn digits(&self) -> &[BigDigit] {
        &self.data
    }
    #[inline]
    fn digits_mut(&mut self) -> &mut Vec<BigDigit> {
        &mut self.data
    }
    #[inline]
    fn normalize(&mut self) {
        self.normalize();
    }
    #[inline]
    fn capacity(&self) -> usize {
        self.data.capacity()
    }
    #[inline]
    fn len(&self) -> usize {
        self.data.len()
    }
}

/// Combine four `u32`s into a single `u128`.
#[cfg(has_i128)]
#[inline]
fn u32_to_u128(a: u32, b: u32, c: u32, d: u32) -> u128 {
    u128::from(d) | (u128::from(c) << 32) | (u128::from(b) << 64) | (u128::from(a) << 96)
}

/// Split a single `u128` into four `u32`.
#[cfg(has_i128)]
#[inline]
fn u32_from_u128(n: u128) -> (u32, u32, u32, u32) {
    (
        (n >> 96) as u32,
        (n >> 64) as u32,
        (n >> 32) as u32,
        n as u32,
    )
}

#[cfg(feature = "serde")]
impl serde::Serialize for BigUint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Note: do not change the serialization format, or it may break forward
        // and backward compatibility of serialized data!  If we ever change the
        // internal representation, we should still serialize in base-`u32`.
        let data: &Vec<u32> = &self.data;
        data.serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for BigUint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data: Vec<u32> = Vec::deserialize(deserializer)?;
        Ok(BigUint::new(data))
    }
}

/// Returns the greatest power of the radix <= big_digit::BASE
#[inline]
fn get_radix_base(radix: u32) -> (BigDigit, usize) {
    debug_assert!(
        2 <= radix && radix <= 256,
        "The radix must be within 2...256"
    );
    debug_assert!(!radix.is_power_of_two());

    // To generate this table:
    //    for radix in 2u64..257 {
    //        let mut power = big_digit::BITS / fls(radix as u64);
    //        let mut base = radix.pow(power as u32);
    //
    //        while let Some(b) = base.checked_mul(radix) {
    //            if b > big_digit::MAX {
    //                break;
    //            }
    //            base = b;
    //            power += 1;
    //        }
    //
    //        println!("({:10}, {:2}), // {:2}", base, power, radix);
    //    }
    // and
    //    for radix in 2u64..257 {
    //        let mut power = 64 / fls(radix as u64);
    //        let mut base = radix.pow(power as u32);
    //
    //        while let Some(b) = base.checked_mul(radix) {
    //            base = b;
    //            power += 1;
    //        }
    //
    //        println!("({:20}, {:2}), // {:2}", base, power, radix);
    //    }
    match big_digit::BITS {
        32 => {
            const BASES: [(u32, usize); 257] = [
                (0, 0),
                (0, 0),
                (0, 0),           //  2
                (3486784401, 20), //  3
                (0, 0),           //  4
                (1220703125, 13), //  5
                (2176782336, 12), //  6
                (1977326743, 11), //  7
                (0, 0),           //  8
                (3486784401, 10), //  9
                (1000000000, 9),  // 10
                (2357947691, 9),  // 11
                (429981696, 8),   // 12
                (815730721, 8),   // 13
                (1475789056, 8),  // 14
                (2562890625, 8),  // 15
                (0, 0),           // 16
                (410338673, 7),   // 17
                (612220032, 7),   // 18
                (893871739, 7),   // 19
                (1280000000, 7),  // 20
                (1801088541, 7),  // 21
                (2494357888, 7),  // 22
                (3404825447, 7),  // 23
                (191102976, 6),   // 24
                (244140625, 6),   // 25
                (308915776, 6),   // 26
                (387420489, 6),   // 27
                (481890304, 6),   // 28
                (594823321, 6),   // 29
                (729000000, 6),   // 30
                (887503681, 6),   // 31
                (0, 0),           // 32
                (1291467969, 6),  // 33
                (1544804416, 6),  // 34
                (1838265625, 6),  // 35
                (2176782336, 6),  // 36
                (2565726409, 6),  // 37
                (3010936384, 6),  // 38
                (3518743761, 6),  // 39
                (4096000000, 6),  // 40
                (115856201, 5),   // 41
                (130691232, 5),   // 42
                (147008443, 5),   // 43
                (164916224, 5),   // 44
                (184528125, 5),   // 45
                (205962976, 5),   // 46
                (229345007, 5),   // 47
                (254803968, 5),   // 48
                (282475249, 5),   // 49
                (312500000, 5),   // 50
                (345025251, 5),   // 51
                (380204032, 5),   // 52
                (418195493, 5),   // 53
                (459165024, 5),   // 54
                (503284375, 5),   // 55
                (550731776, 5),   // 56
                (601692057, 5),   // 57
                (656356768, 5),   // 58
                (714924299, 5),   // 59
                (777600000, 5),   // 60
                (844596301, 5),   // 61
                (916132832, 5),   // 62
                (992436543, 5),   // 63
                (0, 0),           // 64
                (1160290625, 5),  // 65
                (1252332576, 5),  // 66
                (1350125107, 5),  // 67
                (1453933568, 5),  // 68
                (1564031349, 5),  // 69
                (1680700000, 5),  // 70
                (1804229351, 5),  // 71
                (1934917632, 5),  // 72
                (2073071593, 5),  // 73
                (2219006624, 5),  // 74
                (2373046875, 5),  // 75
                (2535525376, 5),  // 76
                (2706784157, 5),  // 77
                (2887174368, 5),  // 78
                (3077056399, 5),  // 79
                (3276800000, 5),  // 80
                (3486784401, 5),  // 81
                (3707398432, 5),  // 82
                (3939040643, 5),  // 83
                (4182119424, 5),  // 84
                (52200625, 4),    // 85
                (54700816, 4),    // 86
                (57289761, 4),    // 87
                (59969536, 4),    // 88
                (62742241, 4),    // 89
                (65610000, 4),    // 90
                (68574961, 4),    // 91
                (71639296, 4),    // 92
                (74805201, 4),    // 93
                (78074896, 4),    // 94
                (81450625, 4),    // 95
                (84934656, 4),    // 96
                (88529281, 4),    // 97
                (92236816, 4),    // 98
                (96059601, 4),    // 99
                (100000000, 4),   // 100
                (104060401, 4),   // 101
                (108243216, 4),   // 102
                (112550881, 4),   // 103
                (116985856, 4),   // 104
                (121550625, 4),   // 105
                (126247696, 4),   // 106
                (131079601, 4),   // 107
                (136048896, 4),   // 108
                (141158161, 4),   // 109
                (146410000, 4),   // 110
                (151807041, 4),   // 111
                (157351936, 4),   // 112
                (163047361, 4),   // 113
                (168896016, 4),   // 114
                (174900625, 4),   // 115
                (181063936, 4),   // 116
                (187388721, 4),   // 117
                (193877776, 4),   // 118
                (200533921, 4),   // 119
                (207360000, 4),   // 120
                (214358881, 4),   // 121
                (221533456, 4),   // 122
                (228886641, 4),   // 123
                (236421376, 4),   // 124
                (244140625, 4),   // 125
                (252047376, 4),   // 126
                (260144641, 4),   // 127
                (0, 0),           // 128
                (276922881, 4),   // 129
                (285610000, 4),   // 130
                (294499921, 4),   // 131
                (303595776, 4),   // 132
                (312900721, 4),   // 133
                (322417936, 4),   // 134
                (332150625, 4),   // 135
                (342102016, 4),   // 136
                (352275361, 4),   // 137
                (362673936, 4),   // 138
                (373301041, 4),   // 139
                (384160000, 4),   // 140
                (395254161, 4),   // 141
                (406586896, 4),   // 142
                (418161601, 4),   // 143
                (429981696, 4),   // 144
                (442050625, 4),   // 145
                (454371856, 4),   // 146
                (466948881, 4),   // 147
                (479785216, 4),   // 148
                (492884401, 4),   // 149
                (506250000, 4),   // 150
                (519885601, 4),   // 151
                (533794816, 4),   // 152
                (547981281, 4),   // 153
                (562448656, 4),   // 154
                (577200625, 4),   // 155
                (592240896, 4),   // 156
                (607573201, 4),   // 157
                (623201296, 4),   // 158
                (639128961, 4),   // 159
                (655360000, 4),   // 160
                (671898241, 4),   // 161
                (688747536, 4),   // 162
                (705911761, 4),   // 163
                (723394816, 4),   // 164
                (741200625, 4),   // 165
                (759333136, 4),   // 166
                (777796321, 4),   // 167
                (796594176, 4),   // 168
                (815730721, 4),   // 169
                (835210000, 4),   // 170
                (855036081, 4),   // 171
                (875213056, 4),   // 172
                (895745041, 4),   // 173
                (916636176, 4),   // 174
                (937890625, 4),   // 175
                (959512576, 4),   // 176
                (981506241, 4),   // 177
                (1003875856, 4),  // 178
                (1026625681, 4),  // 179
                (1049760000, 4),  // 180
                (1073283121, 4),  // 181
                (1097199376, 4),  // 182
                (1121513121, 4),  // 183
                (1146228736, 4),  // 184
                (1171350625, 4),  // 185
                (1196883216, 4),  // 186
                (1222830961, 4),  // 187
                (1249198336, 4),  // 188
                (1275989841, 4),  // 189
                (1303210000, 4),  // 190
                (1330863361, 4),  // 191
                (1358954496, 4),  // 192
                (1387488001, 4),  // 193
                (1416468496, 4),  // 194
                (1445900625, 4),  // 195
                (1475789056, 4),  // 196
                (1506138481, 4),  // 197
                (1536953616, 4),  // 198
                (1568239201, 4),  // 199
                (1600000000, 4),  // 200
                (1632240801, 4),  // 201
                (1664966416, 4),  // 202
                (1698181681, 4),  // 203
                (1731891456, 4),  // 204
                (1766100625, 4),  // 205
                (1800814096, 4),  // 206
                (1836036801, 4),  // 207
                (1871773696, 4),  // 208
                (1908029761, 4),  // 209
                (1944810000, 4),  // 210
                (1982119441, 4),  // 211
                (2019963136, 4),  // 212
                (2058346161, 4),  // 213
                (2097273616, 4),  // 214
                (2136750625, 4),  // 215
                (2176782336, 4),  // 216
                (2217373921, 4),  // 217
                (2258530576, 4),  // 218
                (2300257521, 4),  // 219
                (2342560000, 4),  // 220
                (2385443281, 4),  // 221
                (2428912656, 4),  // 222
                (2472973441, 4),  // 223
                (2517630976, 4),  // 224
                (2562890625, 4),  // 225
                (2608757776, 4),  // 226
                (2655237841, 4),  // 227
                (2702336256, 4),  // 228
                (2750058481, 4),  // 229
                (2798410000, 4),  // 230
                (2847396321, 4),  // 231
                (2897022976, 4),  // 232
                (2947295521, 4),  // 233
                (2998219536, 4),  // 234
                (3049800625, 4),  // 235
                (3102044416, 4),  // 236
                (3154956561, 4),  // 237
                (3208542736, 4),  // 238
                (3262808641, 4),  // 239
                (3317760000, 4),  // 240
                (3373402561, 4),  // 241
                (3429742096, 4),  // 242
                (3486784401, 4),  // 243
                (3544535296, 4),  // 244
                (3603000625, 4),  // 245
                (3662186256, 4),  // 246
                (3722098081, 4),  // 247
                (3782742016, 4),  // 248
                (3844124001, 4),  // 249
                (3906250000, 4),  // 250
                (3969126001, 4),  // 251
                (4032758016, 4),  // 252
                (4097152081, 4),  // 253
                (4162314256, 4),  // 254
                (4228250625, 4),  // 255
                (0, 0),           // 256
            ];

            let (base, power) = BASES[radix as usize];
            (base as BigDigit, power)
        }
        64 => {
            const BASES: [(u64, usize); 257] = [
                (0, 0),
                (0, 0),
                (9223372036854775808, 63),  //  2
                (12157665459056928801, 40), //  3
                (4611686018427387904, 31),  //  4
                (7450580596923828125, 27),  //  5
                (4738381338321616896, 24),  //  6
                (3909821048582988049, 22),  //  7
                (9223372036854775808, 21),  //  8
                (12157665459056928801, 20), //  9
                (10000000000000000000, 19), // 10
                (5559917313492231481, 18),  // 11
                (2218611106740436992, 17),  // 12
                (8650415919381337933, 17),  // 13
                (2177953337809371136, 16),  // 14
                (6568408355712890625, 16),  // 15
                (1152921504606846976, 15),  // 16
                (2862423051509815793, 15),  // 17
                (6746640616477458432, 15),  // 18
                (15181127029874798299, 15), // 19
                (1638400000000000000, 14),  // 20
                (3243919932521508681, 14),  // 21
                (6221821273427820544, 14),  // 22
                (11592836324538749809, 14), // 23
                (876488338465357824, 13),   // 24
                (1490116119384765625, 13),  // 25
                (2481152873203736576, 13),  // 26
                (4052555153018976267, 13),  // 27
                (6502111422497947648, 13),  // 28
                (10260628712958602189, 13), // 29
                (15943230000000000000, 13), // 30
                (787662783788549761, 12),   // 31
                (1152921504606846976, 12),  // 32
                (1667889514952984961, 12),  // 33
                (2386420683693101056, 12),  // 34
                (3379220508056640625, 12),  // 35
                (4738381338321616896, 12),  // 36
                (6582952005840035281, 12),  // 37
                (9065737908494995456, 12),  // 38
                (12381557655576425121, 12), // 39
                (16777216000000000000, 12), // 40
                (550329031716248441, 11),   // 41
                (717368321110468608, 11),   // 42
                (929293739471222707, 11),   // 43
                (1196683881290399744, 11),  // 44
                (1532278301220703125, 11),  // 45
                (1951354384207722496, 11),  // 46
                (2472159215084012303, 11),  // 47
                (3116402981210161152, 11),  // 48
                (3909821048582988049, 11),  // 49
                (4882812500000000000, 11),  // 50
                (6071163615208263051, 11),  // 51
                (7516865509350965248, 11),  // 52
                (9269035929372191597, 11),  // 53
                (11384956040305711104, 11), // 54
                (13931233916552734375, 11), // 55
                (16985107389382393856, 11), // 56
                (362033331456891249, 10),   // 57
                (430804206899405824, 10),   // 58
                (511116753300641401, 10),   // 59
                (604661760000000000, 10),   // 60
                (713342911662882601, 10),   // 61
                (839299365868340224, 10),   // 62
                (984930291881790849, 10),   // 63
                (1152921504606846976, 10),  // 64
                (1346274334462890625, 10),  // 65
                (1568336880910795776, 10),  // 66
                (1822837804551761449, 10),  // 67
                (2113922820157210624, 10),  // 68
                (2446194060654759801, 10),  // 69
                (2824752490000000000, 10),  // 70
                (3255243551009881201, 10),  // 71
                (3743906242624487424, 10),  // 72
                (4297625829703557649, 10),  // 73
                (4923990397355877376, 10),  // 74
                (5631351470947265625, 10),  // 75
                (6428888932339941376, 10),  // 76
                (7326680472586200649, 10),  // 77
                (8335775831236199424, 10),  // 78
                (9468276082626847201, 10),  // 79
                (10737418240000000000, 10), // 80
                (12157665459056928801, 10), // 81
                (13744803133596058624, 10), // 82
                (15516041187205853449, 10), // 83
                (17490122876598091776, 10), // 84
                (231616946283203125, 9),    // 85
                (257327417311663616, 9),    // 86
                (285544154243029527, 9),    // 87
                (316478381828866048, 9),    // 88
                (350356403707485209, 9),    // 89
                (387420489000000000, 9),    // 90
                (427929800129788411, 9),    // 91
                (472161363286556672, 9),    // 92
                (520411082988487293, 9),    // 93
                (572994802228616704, 9),    // 94
                (630249409724609375, 9),    // 95
                (692533995824480256, 9),    // 96
                (760231058654565217, 9),    // 97
                (833747762130149888, 9),    // 98
                (913517247483640899, 9),    // 99
                (1000000000000000000, 9),   // 100
                (1093685272684360901, 9),   // 101
                (1195092568622310912, 9),   // 102
                (1304773183829244583, 9),   // 103
                (1423311812421484544, 9),   // 104
                (1551328215978515625, 9),   // 105
                (1689478959002692096, 9),   // 106
                (1838459212420154507, 9),   // 107
                (1999004627104432128, 9),   // 108
                (2171893279442309389, 9),   // 109
                (2357947691000000000, 9),   // 110
                (2558036924386500591, 9),   // 111
                (2773078757450186752, 9),   // 112
                (3004041937984268273, 9),   // 113
                (3251948521156637184, 9),   // 114
                (3517876291919921875, 9),   // 115
                (3802961274698203136, 9),   // 116
                (4108400332687853397, 9),   // 117
                (4435453859151328768, 9),   // 118
                (4785448563124474679, 9),   // 119
                (5159780352000000000, 9),   // 120
                (5559917313492231481, 9),   // 121
                (5987402799531080192, 9),   // 122
                (6443858614676334363, 9),   // 123
                (6930988311686938624, 9),   // 124
                (7450580596923828125, 9),   // 125
                (8004512848309157376, 9),   // 126
                (8594754748609397887, 9),   // 127
                (9223372036854775808, 9),   // 128
                (9892530380752880769, 9),   // 129
                (10604499373000000000, 9),  // 130
                (11361656654439817571, 9),  // 131
                (12166492167065567232, 9),  // 132
                (13021612539908538853, 9),  // 133
                (13929745610903012864, 9),  // 134
                (14893745087865234375, 9),  // 135
                (15916595351771938816, 9),  // 136
                (17001416405572203977, 9),  // 137
                (18151468971815029248, 9),  // 138
                (139353667211683681, 8),    // 139
                (147578905600000000, 8),    // 140
                (156225851787813921, 8),    // 141
                (165312903998914816, 8),    // 142
                (174859124550883201, 8),    // 143
                (184884258895036416, 8),    // 144
                (195408755062890625, 8),    // 145
                (206453783524884736, 8),    // 146
                (218041257467152161, 8),    // 147
                (230193853492166656, 8),    // 148
                (242935032749128801, 8),    // 149
                (256289062500000000, 8),    // 150
                (270281038127131201, 8),    // 151
                (284936905588473856, 8),    // 152
                (300283484326400961, 8),    // 153
                (316348490636206336, 8),    // 154
                (333160561500390625, 8),    // 155
                (350749278894882816, 8),    // 156
                (369145194573386401, 8),    // 157
                (388379855336079616, 8),    // 158
                (408485828788939521, 8),    // 159
                (429496729600000000, 8),    // 160
                (451447246258894081, 8),    // 161
                (474373168346071296, 8),    // 162
                (498311414318121121, 8),    // 163
                (523300059815673856, 8),    // 164
                (549378366500390625, 8),    // 165
                (576586811427594496, 8),    // 166
                (604967116961135041, 8),    // 167
                (634562281237118976, 8),    // 168
                (665416609183179841, 8),    // 169
                (697575744100000000, 8),    // 170
                (731086699811838561, 8),    // 171
                (765997893392859136, 8),    // 172
                (802359178476091681, 8),    // 173
                (840221879151902976, 8),    // 174
                (879638824462890625, 8),    // 175
                (920664383502155776, 8),    // 176
                (963354501121950081, 8),    // 177
                (1007766734259732736, 8),   // 178
                (1053960288888713761, 8),   // 179
                (1101996057600000000, 8),   // 180
                (1151936657823500641, 8),   // 181
                (1203846470694789376, 8),   // 182
                (1257791680575160641, 8),   // 183
                (1313840315232157696, 8),   // 184
                (1372062286687890625, 8),   // 185
                (1432529432742502656, 8),   // 186
                (1495315559180183521, 8),   // 187
                (1560496482665168896, 8),   // 188
                (1628150074335205281, 8),   // 189
                (1698356304100000000, 8),   // 190
                (1771197285652216321, 8),   // 191
                (1846757322198614016, 8),   // 192
                (1925122952918976001, 8),   // 193
                (2006383000160502016, 8),   // 194
                (2090628617375390625, 8),   // 195
                (2177953337809371136, 8),   // 196
                (2268453123948987361, 8),   // 197
                (2362226417735475456, 8),   // 198
                (2459374191553118401, 8),   // 199
                (2560000000000000000, 8),   // 200
                (2664210032449121601, 8),   // 201
                (2772113166407885056, 8),   // 202
                (2883821021683985761, 8),   // 203
                (2999448015365799936, 8),   // 204
                (3119111417625390625, 8),   // 205
                (3242931408352297216, 8),   // 206
                (3371031134626313601, 8),   // 207
                (3503536769037500416, 8),   // 208
                (3640577568861717121, 8),   // 209
                (3782285936100000000, 8),   // 210
                (3928797478390152481, 8),   // 211
                (4080251070798954496, 8),   // 212
                (4236788918503437921, 8),   // 213
                (4398556620369715456, 8),   // 214
                (4565703233437890625, 8),   // 215
                (4738381338321616896, 8),   // 216
                (4916747105530914241, 8),   // 217
                (5100960362726891776, 8),   // 218
                (5291184662917065441, 8),   // 219
                (5487587353600000000, 8),   // 220
                (5690339646868044961, 8),   // 221
                (5899616690476974336, 8),   // 222
                (6115597639891380481, 8),   // 223
                (6338465731314712576, 8),   // 224
                (6568408355712890625, 8),   // 225
                (6805617133840466176, 8),   // 226
                (7050287992278341281, 8),   // 227
                (7302621240492097536, 8),   // 228
                (7562821648920027361, 8),   // 229
                (7831098528100000000, 8),   // 230
                (8107665808844335041, 8),   // 231
                (8392742123471896576, 8),   // 232
                (8686550888106661441, 8),   // 233
                (8989320386052055296, 8),   // 234
                (9301283852250390625, 8),   // 235
                (9622679558836781056, 8),   // 236
                (9953750901796946721, 8),   // 237
                (10294746488738365696, 8),  // 238
                (10645920227784266881, 8),  // 239
                (11007531417600000000, 8),  // 240
                (11379844838561358721, 8),  // 241
                (11763130845074473216, 8),  // 242
                (12157665459056928801, 8),  // 243
                (12563730464589807616, 8),  // 244
                (12981613503750390625, 8),  // 245
                (13411608173635297536, 8),  // 246
                (13854014124583882561, 8),  // 247
                (14309137159611744256, 8),  // 248
                (14777289335064248001, 8),  // 249
                (15258789062500000000, 8),  // 250
                (15753961211814252001, 8),  // 251
                (16263137215612256256, 8),  // 252
                (16786655174842630561, 8),  // 253
                (17324859965700833536, 8),  // 254
                (17878103347812890625, 8),  // 255
                (72057594037927936, 7),     // 256
            ];

            let (base, power) = BASES[radix as usize];
            (base as BigDigit, power)
        }
        _ => panic!("Invalid bigdigit size"),
    }
}

#[test]
fn test_from_slice() {
    fn check(slice: &[BigDigit], data: &[BigDigit]) {
        assert!(BigUint::from_slice(slice).data == data);
    }
    check(&[1], &[1]);
    check(&[0, 0, 0], &[]);
    check(&[1, 2, 0, 0], &[1, 2]);
    check(&[0, 0, 1, 2], &[0, 0, 1, 2]);
    check(&[0, 0, 1, 2, 0, 0], &[0, 0, 1, 2]);
    check(&[-1i32 as BigDigit], &[-1i32 as BigDigit]);
}

#[test]
fn test_assign_from_slice() {
    fn check(slice: &[BigDigit], data: &[BigDigit]) {
        let mut p = BigUint::from_slice(&[2627_u32, 0_u32, 9182_u32, 42_u32]);
        p.assign_from_slice(slice);
        assert!(p.data == data);
    }
    check(&[1], &[1]);
    check(&[0, 0, 0], &[]);
    check(&[1, 2, 0, 0], &[1, 2]);
    check(&[0, 0, 1, 2], &[0, 0, 1, 2]);
    check(&[0, 0, 1, 2, 0, 0], &[0, 0, 1, 2]);
    check(&[-1i32 as BigDigit], &[-1i32 as BigDigit]);
}

#[cfg(has_i128)]
#[test]
fn test_u32_u128() {
    assert_eq!(u32_from_u128(0u128), (0, 0, 0, 0));
    assert_eq!(
        u32_from_u128(u128::max_value()),
        (
            u32::max_value(),
            u32::max_value(),
            u32::max_value(),
            u32::max_value()
        )
    );

    assert_eq!(
        u32_from_u128(u32::max_value() as u128),
        (0, 0, 0, u32::max_value())
    );

    assert_eq!(
        u32_from_u128(u64::max_value() as u128),
        (0, 0, u32::max_value(), u32::max_value())
    );

    assert_eq!(
        u32_from_u128((u64::max_value() as u128) + u32::max_value() as u128),
        (0, 1, 0, u32::max_value() - 1)
    );

    assert_eq!(u32_from_u128(36_893_488_151_714_070_528), (0, 2, 1, 0));
}

#[cfg(has_i128)]
#[test]
fn test_u128_u32_roundtrip() {
    // roundtrips
    let values = vec![
        0u128,
        1u128,
        u64::max_value() as u128 * 3,
        u32::max_value() as u128,
        u64::max_value() as u128,
        (u64::max_value() as u128) + u32::max_value() as u128,
        u128::max_value(),
    ];

    for val in &values {
        let (a, b, c, d) = u32_from_u128(*val);
        assert_eq!(u32_to_u128(a, b, c, d), *val);
    }
}

#[test]
fn test_pow_biguint() {
    let base = BigUint::from(5u8);
    let exponent = BigUint::from(3u8);

    assert_eq!(BigUint::from(125u8), base.pow(exponent));
}
