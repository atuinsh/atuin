//! Type trait for the mantissa of an extended float.

use crate::util::*;

/// Type trait for the mantissa type.
pub trait Mantissa: UnsignedInteger {
    /// Mask for the left-most bit, to check if the value is normalized.
    const NORMALIZED_MASK: Self;
    /// Mask to extract the high bits from the integer.
    const HIMASK: Self;
    /// Mask to extract the low bits from the integer.
    const LOMASK: Self;
    /// Full size of the integer, in bits.
    const FULL: i32 = Self::BITS as i32;
    /// Half size of the integer, in bits.
    const HALF: i32 = Self::FULL / 2;
}

impl Mantissa for u8 {
    const NORMALIZED_MASK: u8  = 0x80;
    const HIMASK: u8           = 0xF0;
    const LOMASK: u8           = 0x0F;
}

impl Mantissa for u16 {
    const NORMALIZED_MASK: u16  = 0x8000;
    const HIMASK: u16           = 0xFF00;
    const LOMASK: u16           = 0x00FF;
}

impl Mantissa for u32 {
    const NORMALIZED_MASK: u32  = 0x80000000;
    const HIMASK: u32           = 0xFFFF0000;
    const LOMASK: u32           = 0x0000FFFF;
}

impl Mantissa for u64 {
    const NORMALIZED_MASK: u64  = 0x8000000000000000;
    const HIMASK: u64           = 0xFFFFFFFF00000000;
    const LOMASK: u64           = 0x00000000FFFFFFFF;
}

impl Mantissa for u128 {
    const NORMALIZED_MASK: u128 = 0x80000000000000000000000000000000;
    const HIMASK: u128          = 0xFFFFFFFFFFFFFFFF0000000000000000;
    const LOMASK: u128          = 0x0000000000000000FFFFFFFFFFFFFFFF;
}
