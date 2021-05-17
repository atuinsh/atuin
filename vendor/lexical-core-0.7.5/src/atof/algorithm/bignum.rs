//! Big integer type definition.

use crate::float::*;
use crate::util::*;
use super::math::*;

// DATA TYPE

cfg_if! {
if #[cfg(feature = "radix")] {
    use crate::lib::Vec;
    type IntStorageType = Vec<Limb>;
} else {
    // Maximum denominator is 767 mantissa digits + 324 exponent,
    // or 1091 digits, or approximately 3600 bits (round up to 4k).
    #[cfg(limb_width_32)]
    type IntStorageType = arrayvec::ArrayVec<[Limb; 128]>;

    #[cfg(limb_width_64)]
    type IntStorageType = arrayvec::ArrayVec<[Limb; 64]>;
}}  // cfg_if

perftools_inline!{
/// Calculate the integral ceiling of the binary factor from a basen number.
pub(super) fn integral_binary_factor(radix: u32)
    -> u32
{
    debug_assert_radix!(radix);

    #[cfg(not(feature = "radix"))] {
        4
    }

    #[cfg(feature = "radix")] {
        match radix.as_i32() {
            2  => 1,
            3  => 2,
            4  => 2,
            5  => 3,
            6  => 3,
            7  => 3,
            8  => 3,
            9  => 4,
            10 => 4,
            11 => 4,
            12 => 4,
            13 => 4,
            14 => 4,
            15 => 4,
            16 => 4,
            17 => 5,
            18 => 5,
            19 => 5,
            20 => 5,
            21 => 5,
            22 => 5,
            23 => 5,
            24 => 5,
            25 => 5,
            26 => 5,
            27 => 5,
            28 => 5,
            29 => 5,
            30 => 5,
            31 => 5,
            32 => 5,
            33 => 6,
            34 => 6,
            35 => 6,
            36 => 6,
            // Invalid radix
            _  => unreachable!(),
        }
    }
}}

// BIGINT

/// Storage for a big integer type.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub(crate) struct Bigint {
    /// Internal storage for the Bigint, in little-endian order.
    pub(crate) data: IntStorageType,
}

impl Default for Bigint {
    fn default() -> Self {
        // We want to avoid lower-order
        let mut bigint = Bigint { data: IntStorageType::default() };
        bigint.data.reserve(20);
        bigint
    }
}

impl SharedOps for Bigint {
    type StorageType = IntStorageType;

    perftools_inline_always!{
    fn data<'a>(&'a self) -> &'a Self::StorageType {
        &self.data
    }}

    perftools_inline_always!{
    fn data_mut<'a>(&'a mut self) -> &'a mut Self::StorageType {
        &mut self.data
    }}
}

impl SmallOps for Bigint {
}

impl LargeOps for Bigint {
}

// BIGFLOAT

// Adjust the storage capacity for the underlying array.
cfg_if! {
if #[cfg(limb_width_64)] {
    type FloatStorageType = arrayvec::ArrayVec<[Limb; 20]>;
} else {
    type FloatStorageType = arrayvec::ArrayVec<[Limb; 36]>;
}}   // cfg_if

/// Storage for a big floating-point type.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub struct Bigfloat {
    /// Internal storage for the Bigint, in little-endian order.
    ///
    /// Enough storage for up to 10^345, which is 2^1146, or more than
    /// the max for f64.
    pub(crate) data: FloatStorageType,
    /// It also makes sense to store an exponent, since this simplifies
    /// normalizing and powers of 2.
    pub(crate) exp: i32,
}

impl Default for Bigfloat {
    perftools_inline!{
    fn default() -> Self {
        // We want to avoid lower-order
        let mut bigfloat = Bigfloat { data: FloatStorageType::default(), exp: 0 };
        bigfloat.data.reserve(10);
        bigfloat
    }}
}

impl SharedOps for Bigfloat {
    type StorageType = FloatStorageType;

    perftools_inline_always!{
    fn data<'a>(&'a self) -> &'a Self::StorageType {
        &self.data
    }}

    perftools_inline_always!{
    fn data_mut<'a>(&'a mut self) -> &'a mut Self::StorageType {
        &mut self.data
    }}
}

impl SmallOps for Bigfloat {
    perftools_inline!{
    fn imul_pow2(&mut self, n: u32) {
        // Increment exponent to simulate actual multiplication.
        self.exp += n.as_i32();
    }}
}

impl LargeOps for Bigfloat {
}

// TO BIGFLOAT

/// Simple overloads to allow conversions of extended floats to big integers.
pub trait ToBigfloat<M: Mantissa> {
    fn to_bigfloat(&self) -> Bigfloat;
}

impl ToBigfloat<u32> for ExtendedFloat<u32> {
    perftools_inline!{
    fn to_bigfloat(&self) -> Bigfloat {
        let mut bigfloat = Bigfloat::from_u32(self.mant);
        bigfloat.exp = self.exp;
        bigfloat
    }}
}

impl ToBigfloat<u64> for ExtendedFloat<u64> {
    perftools_inline!{
    fn to_bigfloat(&self) -> Bigfloat {
        let mut bigfloat = Bigfloat::from_u64(self.mant);
        bigfloat.exp = self.exp;
        bigfloat
    }}
}

impl ToBigfloat<u128> for ExtendedFloat<u128> {
    perftools_inline!{
    fn to_bigfloat(&self) -> Bigfloat {
        let mut bigfloat = Bigfloat::from_u128(self.mant);
        bigfloat.exp = self.exp;
        bigfloat
    }}
}

// TESTS
// -----

#[cfg(all(test, feature = "correct", feature = "radix"))]
mod test {
    use super::*;

    #[test]
    fn integral_binary_factor_test() {
        const TABLE: [u32; 35] = [1, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6];
        for (idx, base) in (2..37).enumerate() {
            assert_eq!(integral_binary_factor(base), TABLE[idx]);
        }
    }
}
