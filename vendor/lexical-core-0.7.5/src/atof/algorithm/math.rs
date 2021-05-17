//! Building-blocks for arbitrary-precision math.
//!
//! These algorithms assume little-endian order for the large integer
//! buffers, so for a `vec![0, 1, 2, 3]`, `3` is the most significant limb,
//! and `0` is the least significant limb.

// We have a lot of higher-order, efficient math routines that may
// come in handy later. These aren't trivial to implement, so keep them.
#![allow(dead_code)]

use crate::util::*;

// ALIASES
// -------

//  Type for a single limb of the big integer.
//
//  A limb is analogous to a digit in base10, except, it stores 32-bit
//  or 64-bit numbers instead.
//
//  This should be all-known 64-bit platforms supported by Rust.
//      https://forge.rust-lang.org/platform-support.html
//
//  Platforms where native 128-bit multiplication is explicitly supported:
//      - x86_64 (Supported via `MUL`).
//      - mips64 (Supported via `DMULTU`, which `HI` and `LO` can be read-from).
//
//  Platforms where native 64-bit multiplication is supported and
//  you can extract hi-lo for 64-bit multiplications.
//      aarch64 (Requires `UMULH` and `MUL` to capture high and low bits).
//      powerpc64 (Requires `MULHDU` and `MULLD` to capture high and low bits).
//
//  Platforms where native 128-bit multiplication is not supported,
//  requiring software emulation.
//      sparc64 (`UMUL` only supported double-word arguments).
cfg_if! {
if #[cfg(limb_width_64)] {
    pub type Limb = u64;
    type Wide = u128;
    type SignedWide = i128;
} else {
    pub type Limb = u32;
    type Wide = u64;
    type SignedWide = i64;
}}   // cfg_if

perftools_inline!{
/// Cast to limb type.
pub(super) fn as_limb<T: Integer>(t: T)
    -> Limb
{
    as_cast(t)
}}

perftools_inline!{
/// Cast to wide type.
fn as_wide<T: Integer>(t: T)
    -> Wide
{
    as_cast(t)
}}

perftools_inline!{
/// Cast tosigned wide type.
fn as_signed_wide<T: Integer>(t: T)
    -> SignedWide
{
    as_cast(t)
}}

// SPLIT
// -----

perftools_inline!{
/// Split u16 into limbs, in little-endian order.
fn split_u16(x: u16) -> [Limb; 1] {
    [as_limb(x)]
}}

perftools_inline!{
/// Split u32 into limbs, in little-endian order.
fn split_u32(x: u32) -> [Limb; 1] {
    [as_limb(x)]
}}

perftools_inline!{
/// Split u64 into limbs, in little-endian order.
#[cfg(limb_width_32)]
fn split_u64(x: u64) -> [Limb; 2] {
    [as_limb(x), as_limb(x >> 32)]
}}

perftools_inline!{
/// Split u64 into limbs, in little-endian order.
#[cfg(limb_width_64)]
fn split_u64(x: u64) -> [Limb; 1] {
    [as_limb(x)]
}}

perftools_inline!{
/// Split u128 into limbs, in little-endian order.
#[cfg(limb_width_32)]
fn split_u128(x: u128) -> [Limb; 4] {
    [as_limb(x), as_limb(x >> 32), as_limb(x >> 64), as_limb(x >> 96)]
}}

perftools_inline!{
/// Split u128 into limbs, in little-endian order.
#[cfg(limb_width_64)]
fn split_u128(x: u128) -> [Limb; 2] {
    [as_limb(x), as_limb(x >> 64)]
}}

// HI BITS
// -------

// NONZERO

perftools_inline!{
/// Check if any of the remaining bits are non-zero.
pub fn nonzero<T: Integer>(x: &[T], rindex: usize) -> bool {
    let len = x.len();
    let slc = &x[..len-rindex];
    slc.iter().rev().any(|&x| x != T::ZERO)
}}

// HI16

perftools_inline!{
/// Shift 16-bit integer to high 16-bits.
fn u16_to_hi16_1(r0: u16) -> (u16, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    (r0 << ls, false)
}}

perftools_inline!{
/// Shift 2 16-bit integers to high 16-bits.
fn u16_to_hi16_2(r0: u16, r1: u16) -> (u16, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    let rs = 16 - ls;
    let v = match ls {
        0 => r0,
        _ => (r0 << ls) | (r1 >> rs),
    };
    let n = r1 << ls != 0;
    (v, n)
}}

perftools_inline!{
/// Shift 32-bit integer to high 16-bits.
fn u32_to_hi16_1(r0: u32) -> (u16, bool) {
    let r0 = u32_to_hi32_1(r0).0;
    ((r0 >> 16).as_u16(), r0.as_u16() != 0)
}}

perftools_inline!{
/// Shift 2 32-bit integers to high 16-bits.
fn u32_to_hi16_2(r0: u32, r1: u32) -> (u16, bool) {
    let (r0, n) = u32_to_hi32_2(r0, r1);
    ((r0 >> 16).as_u16(), n || r0.as_u16() != 0)
}}

perftools_inline!{
/// Shift 64-bit integer to high 16-bits.
fn u64_to_hi16_1(r0: u64) -> (u16, bool) {
    let r0 = u64_to_hi64_1(r0).0;
    ((r0 >> 48).as_u16(), r0.as_u16() != 0)
}}

perftools_inline!{
/// Shift 2 64-bit integers to high 16-bits.
fn u64_to_hi16_2(r0: u64, r1: u64) -> (u16, bool) {
    let (r0, n) = u64_to_hi64_2(r0, r1);
    ((r0 >> 48).as_u16(), n || r0.as_u16() != 0)
}}

/// Trait to export the high 16-bits from a little-endian slice.
trait Hi16<T>: SliceLike<T> {
    /// Get the hi16 bits from a 1-limb slice.
    fn hi16_1(&self) -> (u16, bool);

    /// Get the hi16 bits from a 2-limb slice.
    fn hi16_2(&self) -> (u16, bool);

    perftools_inline!{
    /// High-level exporter to extract the high 16 bits from a little-endian slice.
    fn hi16(&self) -> (u16, bool) {
        match self.len() {
            0 => (0, false),
            1 => self.hi16_1(),
            _ => self.hi16_2(),
        }
    }}
}

impl Hi16<u16> for [u16] {
    perftools_inline!{
    fn hi16_1(&self) -> (u16, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u16_to_hi16_1(r0)
    }}

    perftools_inline!{
    fn hi16_2(&self) -> (u16, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u16_to_hi16_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}
}

impl Hi16<u32> for [u32] {
    perftools_inline!{
    fn hi16_1(&self) -> (u16, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u32_to_hi16_1(r0)
    }}

    perftools_inline!{
    fn hi16_2(&self) -> (u16, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u32_to_hi16_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}
}

impl Hi16<u64> for [u64] {
    perftools_inline!{
    fn hi16_1(&self) -> (u16, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u64_to_hi16_1(r0)
    }}

    perftools_inline!{
    fn hi16_2(&self) -> (u16, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u64_to_hi16_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}
}

// HI32

perftools_inline!{
/// Shift 32-bit integer to high 32-bits.
fn u32_to_hi32_1(r0: u32) -> (u32, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    (r0 << ls, false)
}}

perftools_inline!{
/// Shift 2 32-bit integers to high 32-bits.
fn u32_to_hi32_2(r0: u32, r1: u32) -> (u32, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    let rs = 32 - ls;
    let v = match ls {
        0 => r0,
        _ => (r0 << ls) | (r1 >> rs),
    };
    let n = r1 << ls != 0;
    (v, n)
}}

perftools_inline!{
/// Shift 64-bit integer to high 32-bits.
fn u64_to_hi32_1(r0: u64) -> (u32, bool) {
    let r0 = u64_to_hi64_1(r0).0;
    ((r0 >> 32).as_u32(), r0.as_u32() != 0)
}}

perftools_inline!{
/// Shift 2 64-bit integers to high 32-bits.
fn u64_to_hi32_2(r0: u64, r1: u64) -> (u32, bool) {
    let (r0, n) = u64_to_hi64_2(r0, r1);
    ((r0 >> 32).as_u32(), n || r0.as_u32() != 0)
}}

/// Trait to export the high 32-bits from a little-endian slice.
trait Hi32<T>: SliceLike<T> {
    /// Get the hi32 bits from a 1-limb slice.
    fn hi32_1(&self) -> (u32, bool);

    /// Get the hi32 bits from a 2-limb slice.
    fn hi32_2(&self) -> (u32, bool);

    /// Get the hi32 bits from a 3-limb slice.
    fn hi32_3(&self) -> (u32, bool);

    perftools_inline!{
    /// High-level exporter to extract the high 32 bits from a little-endian slice.
    fn hi32(&self) -> (u32, bool) {
        match self.len() {
            0 => (0, false),
            1 => self.hi32_1(),
            2 => self.hi32_2(),
            _ => self.hi32_3(),
        }
    }}
}

impl Hi32<u16> for [u16] {
    perftools_inline!{
    fn hi32_1(&self) -> (u32, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        u32_to_hi32_1(rview[0].as_u32())
    }}

    perftools_inline!{
    fn hi32_2(&self) -> (u32, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u32() << 16;
        let r1 = rview[1].as_u32();
        u32_to_hi32_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi32_3(&self) -> (u32, bool) {
        debug_assert!(self.len() >= 3);
        let rview = self.rview();
        let r0 = rview[0].as_u32();
        let r1 = rview[1].as_u32() << 16;
        let r2 = rview[2].as_u32();
        let (v, n) = u32_to_hi32_2( r0, r1 | r2);
        (v, n || nonzero(self, 3))
    }}
}

impl Hi32<u32> for [u32] {
    perftools_inline!{
    fn hi32_1(&self) -> (u32, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u32_to_hi32_1(r0)
    }}

    perftools_inline!{
    fn hi32_2(&self) -> (u32, bool) {
        debug_assert!(self.len() >= 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u32_to_hi32_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}

    perftools_inline!{
    fn hi32_3(&self) -> (u32, bool) {
        self.hi32_2()
    }}
}

impl Hi32<u64> for [u64] {
    perftools_inline!{
    fn hi32_1(&self) -> (u32, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u64_to_hi32_1(r0)
    }}

    perftools_inline!{
    fn hi32_2(&self) -> (u32, bool) {
        debug_assert!(self.len() >= 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u64_to_hi32_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}

    perftools_inline!{
    fn hi32_3(&self) -> (u32, bool) {
        self.hi32_2()
    }}
}

// HI64

perftools_inline!{
/// Shift 64-bit integer to high 64-bits.
fn u64_to_hi64_1(r0: u64) -> (u64, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    (r0 << ls, false)
}}

perftools_inline!{
/// Shift 2 64-bit integers to high 64-bits.
fn u64_to_hi64_2(r0: u64, r1: u64) -> (u64, bool) {
    debug_assert!(r0 != 0);
    let ls = r0.leading_zeros();
    let rs = 64 - ls;
    let v = match ls {
        0 => r0,
        _ => (r0 << ls) | (r1 >> rs),
    };
    let n = r1 << ls != 0;
    (v, n)
}}

/// Trait to export the high 64-bits from a little-endian slice.
trait Hi64<T>: SliceLike<T> {
    /// Get the hi64 bits from a 1-limb slice.
    fn hi64_1(&self) -> (u64, bool);

    /// Get the hi64 bits from a 2-limb slice.
    fn hi64_2(&self) -> (u64, bool);

    /// Get the hi64 bits from a 3-limb slice.
    fn hi64_3(&self) -> (u64, bool);

    /// Get the hi64 bits from a 4-limb slice.
    fn hi64_4(&self) -> (u64, bool);

    /// Get the hi64 bits from a 5-limb slice.
    fn hi64_5(&self) -> (u64, bool);

    perftools_inline!{
    /// High-level exporter to extract the high 64 bits from a little-endian slice.
    fn hi64(&self) -> (u64, bool) {
        match self.len() {
            0 => (0, false),
            1 => self.hi64_1(),
            2 => self.hi64_2(),
            3 => self.hi64_3(),
            4 => self.hi64_4(),
            _ => self.hi64_5(),
        }
    }}
}

impl Hi64<u16> for [u16] {
    perftools_inline!{
    fn hi64_1(&self) -> (u64, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0].as_u64();
        u64_to_hi64_1(r0)
    }}

    perftools_inline!{
    fn hi64_2(&self) -> (u64, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u64() << 16;
        let r1 = rview[1].as_u64();
        u64_to_hi64_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi64_3(&self) -> (u64, bool) {
        debug_assert!(self.len() == 3);
        let rview = self.rview();
        let r0 = rview[0].as_u64() << 32;
        let r1 = rview[1].as_u64() << 16;
        let r2 = rview[2].as_u64();
        u64_to_hi64_1(r0 | r1 | r2)
    }}

    perftools_inline!{
    fn hi64_4(&self) -> (u64, bool) {
        debug_assert!(self.len() == 4);
        let rview = self.rview();
        let r0 = rview[0].as_u64() << 48;
        let r1 = rview[1].as_u64() << 32;
        let r2 = rview[2].as_u64() << 16;
        let r3 = rview[3].as_u64();
        u64_to_hi64_1(r0 | r1 | r2 | r3)
    }}

    perftools_inline!{
    fn hi64_5(&self) -> (u64, bool) {
        debug_assert!(self.len() >= 5);
        let rview = self.rview();
        let r0 = rview[0].as_u64();
        let r1 = rview[1].as_u64() << 48;
        let r2 = rview[2].as_u64() << 32;
        let r3 = rview[3].as_u64() << 16;
        let r4 = rview[4].as_u64();
        let (v, n) = u64_to_hi64_2(r0, r1 | r2 | r3 | r4);
        (v, n || nonzero(self, 5))
    }}
}

impl Hi64<u32> for [u32] {
    perftools_inline!{
    fn hi64_1(&self) -> (u64, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0].as_u64();
        u64_to_hi64_1(r0)
    }}

    perftools_inline!{
    fn hi64_2(&self) -> (u64, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u64() << 32;
        let r1 = rview[1].as_u64();
        u64_to_hi64_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi64_3(&self) -> (u64, bool) {
        debug_assert!(self.len() >= 3);
        let rview = self.rview();
        let r0 = rview[0].as_u64();
        let r1 = rview[1].as_u64() << 32;
        let r2 = rview[2].as_u64();
        let (v, n) = u64_to_hi64_2(r0, r1 | r2);
        (v, n || nonzero(self, 3))
    }}

    perftools_inline!{
    fn hi64_4(&self) -> (u64, bool) {
        self.hi64_3()
    }}

    perftools_inline!{
    fn hi64_5(&self) -> (u64, bool) {
        self.hi64_3()
    }}
}

impl Hi64<u64> for [u64] {
    perftools_inline!{
    fn hi64_1(&self) -> (u64, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0];
        u64_to_hi64_1(r0)
    }}

    perftools_inline!{
    fn hi64_2(&self) -> (u64, bool) {
        debug_assert!(self.len() >= 2);
        let rview = self.rview();
        let r0 = rview[0];
        let r1 = rview[1];
        let (v, n) = u64_to_hi64_2(r0, r1);
        (v, n || nonzero(self, 2))
    }}

    perftools_inline!{
    fn hi64_3(&self) -> (u64, bool) {
        self.hi64_2()
    }}

    perftools_inline!{
    fn hi64_4(&self) -> (u64, bool) {
        self.hi64_2()
    }}

    perftools_inline!{
    fn hi64_5(&self) -> (u64, bool) {
        self.hi64_2()
    }}
}

// HI128

perftools_inline!{
/// Shift 128-bit integer to high 128-bits.
fn u128_to_hi128_1(r0: u128) -> (u128, bool) {
    let ls = r0.leading_zeros();
    (r0 << ls, false)
}}

perftools_inline!{
/// Shift 2 128-bit integers to high 128-bits.
fn u128_to_hi128_2(r0: u128, r1: u128) -> (u128, bool) {
    let ls = r0.leading_zeros();
    let rs = 128 - ls;
    let v = (r0 << ls) | (r1 >> rs);
    let n = r1 << ls != 0;
    (v, n)
}}

/// Trait to export the high 128-bits from a little-endian slice.
trait Hi128<T>: SliceLike<T> {
    /// Get the hi128 bits from a 1-limb slice.
    fn hi128_1(&self) -> (u128, bool);

    /// Get the hi128 bits from a 2-limb slice.
    fn hi128_2(&self) -> (u128, bool);

    /// Get the hi128 bits from a 3-limb slice.
    fn hi128_3(&self) -> (u128, bool);

    /// Get the hi128 bits from a 4-limb slice.
    fn hi128_4(&self) -> (u128, bool);

    /// Get the hi128 bits from a 5-limb slice.
    fn hi128_5(&self) -> (u128, bool);

    /// Get the hi128 bits from a 5-limb slice.
    fn hi128_6(&self) -> (u128, bool);

    /// Get the hi128 bits from a 5-limb slice.
    fn hi128_7(&self) -> (u128, bool);

    /// Get the hi128 bits from a 5-limb slice.
    fn hi128_8(&self) -> (u128, bool);

    /// Get the hi128 bits from a 5-limb slice.
    fn hi128_9(&self) -> (u128, bool);

    perftools_inline!{
    /// High-level exporter to extract the high 128 bits from a little-endian slice.
    fn hi128(&self) -> (u128, bool) {
        match self.len() {
            0 => (0, false),
            1 => self.hi128_1(),
            2 => self.hi128_2(),
            3 => self.hi128_3(),
            4 => self.hi128_4(),
            6 => self.hi128_6(),
            7 => self.hi128_7(),
            8 => self.hi128_8(),
            _ => self.hi128_9(),
        }
    }}
}

impl Hi128<u16> for [u16] {
    perftools_inline!{
    fn hi128_1(&self) -> (u128, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        u128_to_hi128_1(r0)
    }}

    perftools_inline!{
    fn hi128_2(&self) -> (u128, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 16;
        let r1 = rview[1].as_u128();
        u128_to_hi128_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi128_3(&self) -> (u128, bool) {
        debug_assert!(self.len() == 3);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 32;
        let r1 = rview[1].as_u128() << 16;
        let r2 = rview[2].as_u128();
        u128_to_hi128_1(r0 | r1 | r2)
    }}

    perftools_inline!{
    fn hi128_4(&self) -> (u128, bool) {
        debug_assert!(self.len() == 4);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 48;
        let r1 = rview[1].as_u128() << 32;
        let r2 = rview[2].as_u128() << 16;
        let r3 = rview[3].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3)
    }}

    perftools_inline!{
    fn hi128_5(&self) -> (u128, bool) {
        debug_assert!(self.len() == 5);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 64;
        let r1 = rview[1].as_u128() << 48;
        let r2 = rview[2].as_u128() << 32;
        let r3 = rview[3].as_u128() << 16;
        let r4 = rview[4].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3 | r4)
    }}

    perftools_inline!{
    fn hi128_6(&self) -> (u128, bool) {
        debug_assert!(self.len() == 6);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 80;
        let r1 = rview[1].as_u128() << 64;
        let r2 = rview[2].as_u128() << 48;
        let r3 = rview[3].as_u128() << 32;
        let r4 = rview[4].as_u128() << 16;
        let r5 = rview[5].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3 | r4 | r5)
    }}

    perftools_inline!{
    fn hi128_7(&self) -> (u128, bool) {
        debug_assert!(self.len() == 7);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 96;
        let r1 = rview[1].as_u128() << 80;
        let r2 = rview[2].as_u128() << 64;
        let r3 = rview[3].as_u128() << 48;
        let r4 = rview[4].as_u128() << 32;
        let r5 = rview[5].as_u128() << 16;
        let r6 = rview[6].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3 | r4 | r5 | r6)
    }}

    perftools_inline!{
    fn hi128_8(&self) -> (u128, bool) {
        debug_assert!(self.len() == 8);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 112;
        let r1 = rview[1].as_u128() << 96;
        let r2 = rview[2].as_u128() << 80;
        let r3 = rview[3].as_u128() << 64;
        let r4 = rview[4].as_u128() << 48;
        let r5 = rview[5].as_u128() << 32;
        let r6 = rview[6].as_u128() << 16;
        let r7 = rview[7].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3 | r4 | r5 | r6 | r7)
    }}

    perftools_inline!{
    fn hi128_9(&self) -> (u128, bool) {
        debug_assert!(self.len() >= 9);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        let r1 = rview[1].as_u128() << 112;
        let r2 = rview[2].as_u128() << 96;
        let r3 = rview[3].as_u128() << 80;
        let r4 = rview[4].as_u128() << 64;
        let r5 = rview[5].as_u128() << 48;
        let r6 = rview[6].as_u128() << 32;
        let r7 = rview[7].as_u128() << 16;
        let r8 = rview[8].as_u128();
        let (v, n) = u128_to_hi128_2(r0, r1 | r2 | r3 | r4 | r5 | r6 | r7 | r8);
        (v, n || nonzero(self, 9))
    }}
}

impl Hi128<u32> for [u32] {
    perftools_inline!{
    fn hi128_1(&self) -> (u128, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        u128_to_hi128_1(r0)
    }}

    perftools_inline!{
    fn hi128_2(&self) -> (u128, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 32;
        let r1 = rview[1].as_u128();
        u128_to_hi128_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi128_3(&self) -> (u128, bool) {
        debug_assert!(self.len() == 3);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 64;
        let r1 = rview[1].as_u128() << 32;
        let r2 = rview[2].as_u128();
        u128_to_hi128_1(r0 | r1 | r2)
    }}

    perftools_inline!{
    fn hi128_4(&self) -> (u128, bool) {
        debug_assert!(self.len() == 4);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 96;
        let r1 = rview[1].as_u128() << 64;
        let r2 = rview[2].as_u128() << 32;
        let r3 = rview[3].as_u128();
        u128_to_hi128_1(r0 | r1 | r2 | r3)
    }}

    perftools_inline!{
    fn hi128_5(&self) -> (u128, bool) {
        debug_assert!(self.len() >= 5);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        let r1 = rview[1].as_u128() << 96;
        let r2 = rview[2].as_u128() << 64;
        let r3 = rview[3].as_u128() << 32;
        let r4 = rview[4].as_u128();
        let (v, n) = u128_to_hi128_2(r0, r1 | r2 | r3 | r4);
        (v, n || nonzero(self, 5))
    }}

    perftools_inline!{
    fn hi128_6(&self) -> (u128, bool) {
        self.hi128_5()
    }}

    perftools_inline!{
    fn hi128_7(&self) -> (u128, bool) {
        self.hi128_5()
    }}

    perftools_inline!{
    fn hi128_8(&self) -> (u128, bool) {
        self.hi128_5()
    }}

    perftools_inline!{
    fn hi128_9(&self) -> (u128, bool) {
        self.hi128_5()
    }}
}

impl Hi128<u64> for [u64] {
    perftools_inline!{
    fn hi128_1(&self) -> (u128, bool) {
        debug_assert!(self.len() == 1);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        u128_to_hi128_1(r0)
    }}

    perftools_inline!{
    fn hi128_2(&self) -> (u128, bool) {
        debug_assert!(self.len() == 2);
        let rview = self.rview();
        let r0 = rview[0].as_u128() << 64;
        let r1 = rview[1].as_u128();
        u128_to_hi128_1(r0 | r1)
    }}

    perftools_inline!{
    fn hi128_3(&self) -> (u128, bool) {
        debug_assert!(self.len() >= 3);
        let rview = self.rview();
        let r0 = rview[0].as_u128();
        let r1 = rview[1].as_u128() << 64;
        let r2 = rview[2].as_u128();
        let (v, n) = u128_to_hi128_2(r0, r1 | r2);
        (v, n || nonzero(self, 3))
    }}

    perftools_inline!{
    fn hi128_4(&self) -> (u128, bool) {
        self.hi128_3()
    }}

    perftools_inline!{
    fn hi128_5(&self) -> (u128, bool) {
        self.hi128_3()
    }}

    perftools_inline!{
    fn hi128_6(&self) -> (u128, bool) {
        self.hi128_3()
    }}

    perftools_inline!{
    fn hi128_7(&self) -> (u128, bool) {
        self.hi128_3()
    }}

    perftools_inline!{
    fn hi128_8(&self) -> (u128, bool) {
        self.hi128_3()
    }}

    perftools_inline!{
    fn hi128_9(&self) -> (u128, bool) {
        self.hi128_3()
    }}
}

// SCALAR
// ------

// Scalar-to-scalar operations, for building-blocks for arbitrary-precision
// operations.

pub(in crate::atof::algorithm) mod scalar {

use super::*;

// ADDITION

perftools_inline!{
/// Add two small integers and return the resulting value and if overflow happens.
pub fn add(x: Limb, y: Limb)
    -> (Limb, bool)
{
    x.overflowing_add(y)
}}

perftools_inline!{
/// AddAssign two small integers and return if overflow happens.
pub fn iadd(x: &mut Limb, y: Limb)
    -> bool
{
    let t = add(*x, y);
    *x = t.0;
    t.1
}}

// SUBTRACTION

perftools_inline!{
/// Subtract two small integers and return the resulting value and if overflow happens.
pub fn sub(x: Limb, y: Limb)
    -> (Limb, bool)
{
    x.overflowing_sub(y)
}}

perftools_inline!{
/// SubAssign two small integers and return if overflow happens.
pub fn isub(x: &mut Limb, y: Limb)
    -> bool
{
    let t = sub(*x, y);
    *x = t.0;
    t.1
}}

// MULTIPLICATION

perftools_inline!{
/// Multiply two small integers (with carry) (and return the overflow contribution).
///
/// Returns the (low, high) components.
pub fn mul(x: Limb, y: Limb, carry: Limb)
    -> (Limb, Limb)
{
    // Cannot overflow, as long as wide is 2x as wide. This is because
    // the following is always true:
    // `Wide::max_value() - (Narrow::max_value() * Narrow::max_value()) >= Narrow::max_value()`
    let z: Wide = as_wide(x) * as_wide(y) + as_wide(carry);
    (as_limb(z), as_limb(z >> <Limb as Integer>::BITS))
}}

perftools_inline!{
/// Multiply two small integers (with carry) (and return if overflow happens).
pub fn imul(x: &mut Limb, y: Limb, carry: Limb)
    -> Limb
{
    let t = mul(*x, y, carry);
    *x = t.0;
    t.1
}}

// DIVISION

perftools_inline!{
/// Divide two small integers (with remainder) (and return the remainder contribution).
///
/// Returns the (value, remainder) components.
pub fn div(x: Limb, y: Limb, rem: Limb)
    -> (Limb, Limb)
{
    // Cannot overflow, as long as wide is 2x as wide.
    let x = as_wide(x) | (as_wide(rem) << <Limb as Integer>::BITS);
    let y = as_wide(y);
    (as_limb(x / y), as_limb(x % y))
}}

perftools_inline!{
/// DivAssign two small integers and return the remainder.
pub fn idiv(x: &mut Limb, y: Limb, rem: Limb)
    -> Limb
{
    let t = div(*x, y, rem);
    *x = t.0;
    t.1
}}

}   // scalar

// SMALL
// -----

// Large-to-small operations, to modify a big integer from a native scalar.

pub(in crate::atof::algorithm) mod small {

use crate::lib::iter;
use super::*;
use super::super::small_powers::*;
use super::super::large_powers::*;

// PROPERTIES

perftools_inline!{
/// Get the number of leading zero values in the storage.
/// Assumes the value is normalized.
pub fn leading_zero_limbs(_: &[Limb]) -> usize {
    0
}}

perftools_inline!{
/// Get the number of trailing zero values in the storage.
/// Assumes the value is normalized.
pub fn trailing_zero_limbs(x: &[Limb]) -> usize {
    let mut iter = x.iter().enumerate();
    let opt = iter.find(|&tup| !tup.1.is_zero());
    let value = opt
        .map(|t| t.0)
        .unwrap_or(x.len());

    value
}}

perftools_inline!{
/// Get number of leading zero bits in the storage.
pub fn leading_zeros(x: &[Limb]) -> usize {
    if x.is_empty() {
        0
    } else {
        x.rindex(0).leading_zeros().as_usize()
    }
}}

perftools_inline!{
/// Get number of trailing zero bits in the storage.
/// Assumes the value is normalized.
pub fn trailing_zeros(x: &[Limb]) -> usize {
    // Get the index of the last non-zero value
    let index = trailing_zero_limbs(x);
    let mut count = index.saturating_mul(<Limb as Integer>::BITS);
    if let Some(value) = x.get(index) {
        count = count.saturating_add(value.trailing_zeros().as_usize());
    }
    count
}}

// BIT LENGTH

perftools_inline!{
/// Calculate the bit-length of the big-integer.
pub fn bit_length(x: &[Limb]) -> usize {
    // Avoid overflowing, calculate via total number of bits
    // minus leading zero bits.
    let nlz = leading_zeros(x);
    <Limb as Integer>::BITS.checked_mul(x.len())
        .map(|v| v - nlz)
        .unwrap_or(usize::max_value())
}}

// BIT LENGTH

perftools_inline!{
/// Calculate the limb-length of the big-integer.
pub fn limb_length(x: &[Limb]) -> usize {
    x.len()
}}

// SHR

perftools_inline!{
/// Shift-right bits inside a buffer and returns the truncated bits.
///
/// Returns the truncated bits.
///
/// Assumes `n < <Limb as Integer>::BITS`, IE, internally shifting bits.
pub fn ishr_bits<T>(x: &mut T, n: usize)
    -> Limb
    where T: CloneableVecLike<Limb>
{
    // Need to shift by the number of `bits % <Limb as Integer>::BITS`.
    let bits = <Limb as Integer>::BITS;
    debug_assert!(n < bits && n != 0);

    // Internally, for each item, we shift left by n, and add the previous
    // right shifted limb-bits.
    // For example, we transform (for u8) shifted right 2, to:
    //      b10100100 b01000010
    //        b101001 b00010000
    let lshift = bits - n;
    let rshift = n;
    let mut prev: Limb = 0;
    for xi in x.iter_mut().rev() {
        let tmp = *xi;
        *xi >>= rshift;
        *xi |= prev << lshift;
        prev = tmp;
    }

    prev & lower_n_mask(as_limb(rshift))
}}

perftools_inline!{
/// Shift-right `n` limbs inside a buffer and returns if all the truncated limbs are zero.
///
/// Assumes `n` is not 0.
pub fn ishr_limbs<T>(x: &mut T, n: usize)
    -> bool
    where T: CloneableVecLike<Limb>
{
    debug_assert!(n != 0);

    if n >= x.len() {
        x.clear();
        false
    } else {
        let is_zero = (&x[..n]).iter().all(|v| v.is_zero());
        x.remove_many(0..n);
        is_zero
    }
}}

perftools_inline!{
/// Shift-left buffer by n bits and return if we should round-up.
pub fn ishr<T>(x: &mut T, n: usize)
    -> bool
    where T: CloneableVecLike<Limb>
{
    let bits = <Limb as Integer>::BITS;
    // Need to pad with zeros for the number of `bits / <Limb as Integer>::BITS`,
    // and shift-left with carry for `bits % <Limb as Integer>::BITS`.
    let rem = n % bits;
    let div = n / bits;
    let is_zero = match div.is_zero() {
        true  => true,
        false => ishr_limbs(x, div),
    };
    let truncated = match rem.is_zero() {
        true  => 0,
        false => ishr_bits(x, rem),
    };

    // Calculate if we need to roundup.
    let roundup = {
        let halfway = lower_n_halfway(as_limb(rem));
        if truncated > halfway {
            // Above halfway
            true
        } else if truncated == halfway {
            // Exactly halfway, if !is_zero, we have a tie-breaker,
            // otherwise, we follow round-to-nearest, tie-even rules.
            // Cannot be empty, since truncated is non-zero.
            !is_zero || x[0].is_odd()
        } else {
            // Below halfway
            false
        }
    };

    // Normalize the data
    normalize(x);

    roundup
}}

perftools_inline!{
/// Shift-left buffer by n bits.
pub fn shr<T>(x: &[Limb], n: usize)
    -> (T, bool)
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    let roundup = ishr(&mut z, n);
    (z, roundup)
}}

// SHL

perftools_inline!{
/// Shift-left bits inside a buffer.
///
/// Assumes `n < <Limb as Integer>::BITS`, IE, internally shifting bits.
pub fn ishl_bits<T>(x: &mut T, n: usize)
    where T: CloneableVecLike<Limb>
{
    // Need to shift by the number of `bits % <Limb as Integer>::BITS)`.
    let bits = <Limb as Integer>::BITS;
    debug_assert!(n < bits);
    if n.is_zero() {
        return;
    }

    // Internally, for each item, we shift left by n, and add the previous
    // right shifted limb-bits.
    // For example, we transform (for u8) shifted left 2, to:
    //      b10100100 b01000010
    //      b10 b10010001 b00001000
    let rshift = bits - n;
    let lshift = n;
    let mut prev: Limb = 0;
    for xi in x.iter_mut() {
        let tmp = *xi;
        *xi <<= lshift;
        *xi |= prev >> rshift;
        prev = tmp;
    }

    // Always push the carry, even if it creates a non-normal result.
    let carry = prev >> rshift;
    if carry != 0 {
        x.push(carry);
    }
}}

perftools_inline!{
/// Shift-left bits inside a buffer.
///
/// Assumes `n < <Limb as Integer>::BITS`, IE, internally shifting bits.
pub fn shl_bits<T>(x: &[Limb], n: usize)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    ishl_bits(&mut z, n);
    z
}}

perftools_inline!{
/// Shift-left `n` digits inside a buffer.
///
/// Assumes `n` is not 0.
pub fn ishl_limbs<T>(x: &mut T, n: usize)
    where T: CloneableVecLike<Limb>
{
    debug_assert!(n != 0);
    if !x.is_empty() {
        x.insert_many(0, iter::repeat(0).take(n));
    }
}}

perftools_inline!{
/// Shift-left buffer by n bits.
pub fn ishl<T>(x: &mut T, n: usize)
    where T: CloneableVecLike<Limb>
{
    let bits = <Limb as Integer>::BITS;
    // Need to pad with zeros for the number of `bits / <Limb as Integer>::BITS`,
    // and shift-left with carry for `bits % <Limb as Integer>::BITS`.
    let rem = n % bits;
    let div = n / bits;
    ishl_bits(x, rem);
    if !div.is_zero() {
        ishl_limbs(x, div);
    }
}}

perftools_inline!{
/// Shift-left buffer by n bits.
pub fn shl<T>(x: &[Limb], n: usize)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    ishl(&mut z, n);
    z
}}

// NORMALIZE

perftools_inline!{
/// Normalize the container by popping any leading zeros.
pub fn normalize<T>(x: &mut T)
    where T: CloneableVecLike<Limb>
{
    // Remove leading zero if we cause underflow. Since we're dividing
    // by a small power, we have at max 1 int removed.
    while !x.is_empty() && x.rindex(0).is_zero() {
        x.pop();
    }
}}

// ADDITION

perftools_inline!{
/// Implied AddAssign implementation for adding a small integer to bigint.
///
/// Allows us to choose a start-index in x to store, to allow incrementing
/// from a non-zero start.
pub fn iadd_impl<T>(x: &mut T, y: Limb, xstart: usize)
    where T: CloneableVecLike<Limb>
{
    if x.len() <= xstart {
        x.push(y);
    } else {
        // Initial add
        let mut carry = scalar::iadd(&mut x[xstart], y);

        // Increment until overflow stops occurring.
        let mut size = xstart + 1;
        while carry && size < x.len() {
            carry = scalar::iadd(&mut x[size], 1);
            size += 1;
        }

        // If we overflowed the buffer entirely, need to add 1 to the end
        // of the buffer.
        if carry {
            x.push(1);
        }
    }
}}

perftools_inline!{
/// AddAssign small integer to bigint.
pub fn iadd<T>(x: &mut T, y: Limb)
    where T: CloneableVecLike<Limb>
{
    iadd_impl(x, y, 0);
}}

perftools_inline!{
/// Add small integer to bigint.
pub fn add<T>(x: &[Limb], y: Limb)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    iadd(&mut z, y);
    z
}}

// SUBTRACTION

perftools_inline!{
/// SubAssign small integer to bigint.
/// Does not do overflowing subtraction.
pub fn isub_impl<T>(x: &mut T, y: Limb, xstart: usize)
    where T: CloneableVecLike<Limb>
{
    debug_assert!(x.len() > xstart && (x[xstart] >= y || x.len() > xstart+1));

    // Initial subtraction
    let mut carry = scalar::isub(&mut x[xstart], y);

    // Increment until overflow stops occurring.
    let mut size = xstart + 1;
    while carry && size < x.len() {
        carry = scalar::isub(&mut x[size], 1);
        size += 1;
    }
    normalize(x);
}}

perftools_inline!{
/// SubAssign small integer to bigint.
/// Does not do overflowing subtraction.
pub fn isub<T>(x: &mut T, y: Limb)
    where T: CloneableVecLike<Limb>
{
    isub_impl(x, y, 0);
}}

perftools_inline!{
/// Sub small integer to bigint.
pub fn sub<T>(x: &[Limb], y: Limb)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    isub(&mut z, y);
    z
}}

// MULTIPLICATION

perftools_inline!{
/// MulAssign small integer to bigint.
pub fn imul<T>(x: &mut T, y: Limb)
    where T: CloneableVecLike<Limb>
{
    // Multiply iteratively over all elements, adding the carry each time.
    let mut carry: Limb = 0;
    for xi in x.iter_mut() {
        carry = scalar::imul(xi, y, carry);
    }

    // Overflow of value, add to end.
    if carry != 0 {
        x.push(carry);
    }
}}

perftools_inline!{
/// Mul small integer to bigint.
pub fn mul<T>(x: &[Limb], y: Limb)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    imul(&mut z, y);
    z
}}

/// MulAssign by a power.
///
/// Theoretically...
///
/// Use an exponentiation by squaring method, since it reduces the time
/// complexity of the multiplication to ~`O(log(n))` for the squaring,
/// and `O(n*m)` for the result. Since `m` is typically a lower-order
/// factor, this significantly reduces the number of multiplications
/// we need to do. Iteratively multiplying by small powers follows
/// the nth triangular number series, which scales as `O(p^2)`, but
/// where `p` is `n+m`. In short, it scales very poorly.
///
/// Practically....
///
/// Exponentiation by Squaring:
///     running 2 tests
///     test bigcomp_f32_lexical ... bench:       1,018 ns/iter (+/- 78)
///     test bigcomp_f64_lexical ... bench:       3,639 ns/iter (+/- 1,007)
///
/// Exponentiation by Iterative Small Powers:
///     running 2 tests
///     test bigcomp_f32_lexical ... bench:         518 ns/iter (+/- 31)
///     test bigcomp_f64_lexical ... bench:         583 ns/iter (+/- 47)
///
/// Exponentiation by Iterative Large Powers (of 2):
///     running 2 tests
///     test bigcomp_f32_lexical ... bench:         671 ns/iter (+/- 31)
///     test bigcomp_f64_lexical ... bench:       1,394 ns/iter (+/- 47)
///
/// Even using worst-case scenarios, exponentiation by squaring is
/// significantly slower for our workloads. Just multiply by small powers,
/// in simple cases, and use precalculated large powers in other cases.
pub fn imul_power<T>(x: &mut T, radix: u32, n: u32)
    where T: CloneableVecLike<Limb>
{
    use super::large::KARATSUBA_CUTOFF;

    let small_powers = get_small_powers(radix);
    let large_powers = get_large_powers(radix);

    if n == 0 {
        // No exponent, just return.
        // The 0-index of the large powers is `2^0`, which is 1, so we want
        // to make sure we don't take that path with a literal 0.
        return;
    }

    // We want to use the asymptotically faster algorithm if we're going
    // to be using Karabatsu multiplication sometime during the result,
    // otherwise, just use exponentiation by squaring.
    let bit_length = 32 - n.leading_zeros().as_usize();
    debug_assert!(bit_length != 0 && bit_length <= large_powers.len());
    if x.len() + large_powers[bit_length-1].len() < 2*KARATSUBA_CUTOFF {
        // We can use iterative small powers to make this faster for the
        // easy cases.

        // Multiply by the largest small power until n < step.
        let step = small_powers.len() - 1;
        let power = small_powers[step];
        let mut n = n.as_usize();
        while n >= step {
            imul(x, power);
            n -= step;
        }

        // Multiply by the remainder.
        imul(x, small_powers[n]);
    } else {
        // In theory, this code should be asymptotically a lot faster,
        // in practice, our small::imul seems to be the limiting step,
        // and large imul is slow as well.

        // Multiply by higher order powers.
        let mut idx: usize = 0;
        let mut bit: usize = 1;
        let mut n = n.as_usize();
        while n != 0 {
            if n & bit != 0 {
                debug_assert!(idx < large_powers.len());
                large::imul(x, large_powers[idx]);
                n ^= bit;
            }
            idx += 1;
            bit <<= 1;
        }
    }
}

perftools_inline!{
/// Mul by a power.
pub fn mul_power<T>(x: &[Limb], radix: u32, n: u32)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    imul_power(&mut z, radix, n);
    z
}}

// DIVISION

perftools_inline!{
/// DivAssign small integer to bigint and get the remainder.
pub fn idiv<T>(x: &mut T, y: Limb)
    -> Limb
    where T: CloneableVecLike<Limb>
{
    // Divide iteratively over all elements, adding the carry each time.
    let mut rem: Limb = 0;
    for xi in x.iter_mut().rev() {
        rem = scalar::idiv(xi, y, rem);
    }
    normalize(x);

    rem
}}

perftools_inline!{
/// Div small integer to bigint and get the remainder.
pub fn div<T>(x: &[Limb], y: Limb)
    -> (T, Limb)
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    let rem = idiv(&mut z, y);
    (z, rem)
}}

// POWER

perftools_inline!{
/// Calculate x^n, using exponentiation by squaring.
///
/// This algorithm is slow, using `mul_power` should generally be preferred,
/// as although it's not asymptotically faster, it precalculates a lot
/// of results.
pub fn ipow<T>(x: &mut T, mut n: Limb)
    where T: CloneableVecLike<Limb>
{
    // Store `x` as 1, and switch `base` to `x`.
    let mut base = T::default();
    base.push(1);
    mem::swap(x, &mut base);

    // Do main algorithm.
    loop {
        if n.is_odd() {
            large::imul(x, &base);
        }
        n /= 2;

        // We need to break as a post-condition, since the real work
        // is in the `imul` and `mul` algorithms.
        if n.is_zero() {
            break;
        } else {
            base = large::mul(&base, &base);
        }
    }
}}

perftools_inline!{
/// Calculate x^n, using exponentiation by squaring.
pub fn pow<T>(x: &[Limb], n: Limb)
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    ipow(&mut z, n);
    z
}}

}   // small

// LARGE
// -----

// Large-to-large operations, to modify a big integer from a native scalar.

pub(in crate::atof::algorithm) mod large {

use crate::lib::cmp;
use super::*;

// RELATIVE OPERATORS

perftools_inline!{
/// Compare `x` to `y`, in little-endian order.
pub fn compare(x: &[Limb], y: &[Limb]) -> cmp::Ordering {
    if x.len() > y.len() {
        return cmp::Ordering::Greater;
    } else if x.len() < y.len() {
        return cmp::Ordering::Less;
    } else {
        let iter = x.iter().rev().zip(y.iter().rev());
        for (&xi, &yi) in iter {
            if xi > yi {
                return cmp::Ordering::Greater;
            } else if xi < yi {
                return cmp::Ordering::Less;
            }
        }
        // Equal case.
        return cmp::Ordering::Equal;
    }
}}

perftools_inline!{
/// Check if x is greater than y.
pub fn greater(x: &[Limb], y: &[Limb]) -> bool {
    compare(x, y) == cmp::Ordering::Greater
}}

perftools_inline!{
/// Check if x is greater than or equal to y.
pub fn greater_equal(x: &[Limb], y: &[Limb]) -> bool {
    !less(x, y)
}}

perftools_inline!{
/// Check if x is less than y.
pub fn less(x: &[Limb], y: &[Limb]) -> bool {
    compare(x, y) == cmp::Ordering::Less
}}

perftools_inline!{
/// Check if x is less than or equal to y.
pub fn less_equal(x: &[Limb], y: &[Limb]) -> bool {
    !greater(x, y)
}}

perftools_inline!{
/// Check if x is equal to y.
/// Slightly optimized for equality comparisons, since it reduces the number
/// of comparisons relative to `compare`.
pub fn equal(x: &[Limb], y: &[Limb]) -> bool {
    let mut iter = x.iter().rev().zip(y.iter().rev());
    x.len() == y.len() && iter.all(|(&xi, &yi)| xi == yi)
}}

/// ADDITION

/// Implied AddAssign implementation for bigints.
///
/// Allows us to choose a start-index in x to store, so we can avoid
/// padding the buffer with zeros when not needed, optimized for vectors.
pub fn iadd_impl<T>(x: &mut T, y: &[Limb], xstart: usize)
    where T: CloneableVecLike<Limb>
{
    // The effective x buffer is from `xstart..x.len()`, so we need to treat
    // that as the current range. If the effective y buffer is longer, need
    // to resize to that, + the start index.
    if y.len() > x.len() - xstart {
        x.resize(y.len() + xstart, 0);
    }

    // Iteratively add elements from y to x.
    let mut carry = false;
    for (xi, yi) in (&mut x[xstart..]).iter_mut().zip(y.iter()) {
        // Only one op of the two can overflow, since we added at max
        // Limb::max_value() + Limb::max_value(). Add the previous carry,
        // and store the current carry for the next.
        let mut tmp = scalar::iadd(xi, *yi);
        if carry {
            tmp |= scalar::iadd(xi, 1);
        }
        carry = tmp;
    }

    // Overflow from the previous bit.
    if carry {
        small::iadd_impl(x, 1, y.len()+xstart);
    }
}

perftools_inline!{
/// AddAssign bigint to bigint.
pub fn iadd<T>(x: &mut T, y: &[Limb])
    where T: CloneableVecLike<Limb>
{
    iadd_impl(x, y, 0)
}}

perftools_inline!{
/// Add bigint to bigint.
pub fn add<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    iadd(&mut z, y);
    z
}}

// SUBTRACTION

/// SubAssign bigint to bigint.
pub fn isub<T>(x: &mut T, y: &[Limb])
    where T: CloneableVecLike<Limb>
{
    // Basic underflow checks.
    debug_assert!(greater_equal(x, y));

    // Iteratively add elements from y to x.
    let mut carry = false;
    for (xi, yi) in x.iter_mut().zip(y.iter()) {
        // Only one op of the two can overflow, since we added at max
        // Limb::max_value() + Limb::max_value(). Add the previous carry,
        // and store the current carry for the next.
        let mut tmp = scalar::isub(xi, *yi);
        if carry {
            tmp |= scalar::isub(xi, 1);
        }
        carry = tmp;
    }

    if carry {
        small::isub_impl(x, 1, y.len());
    } else {
        small::normalize(x);
    }
}

perftools_inline!{
/// Sub bigint to bigint.
pub fn sub<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    isub(&mut z, y);
    z
}}

// MULTIPLICATIION

/// Number of digits to bottom-out to asymptotically slow algorithms.
///
/// Karatsuba tends to out-perform long-multiplication at ~320-640 bits,
/// so we go halfway, while Newton division tends to out-perform
/// Algorithm D at ~1024 bits. We can toggle this for optimal performance.
pub const KARATSUBA_CUTOFF: usize = 32;

/// Grade-school multiplication algorithm.
///
/// Slow, naive algorithm, using limb-bit bases and just shifting left for
/// each iteration. This could be optimized with numerous other algorithms,
/// but it's extremely simple, and works in O(n*m) time, which is fine
/// by me. Each iteration, of which there are `m` iterations, requires
/// `n` multiplications, and `n` additions, or grade-school multiplication.
fn long_mul<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    // Using the immutable value, multiply by all the scalars in y, using
    // the algorithm defined above. Use a single buffer to avoid
    // frequent reallocations. Handle the first case to avoid a redundant
    // addition, since we know y.len() >= 1.
    let mut z: T = small::mul(x, y[0]);
    z.resize(x.len() + y.len(), 0);

    // Handle the iterative cases.
    for (i, &yi) in y[1..].iter().enumerate() {
        let zi: T = small::mul(x, yi);
        iadd_impl(&mut z, &zi, i+1);
    }

    small::normalize(&mut z);

    z
}

perftools_inline!{
/// Split two buffers into halfway, into (lo, hi).
pub fn karatsuba_split<'a>(z: &'a [Limb], m: usize)
    -> (&'a [Limb], &'a [Limb])
{
    (&z[..m], &z[m..])
}}

/// Karatsuba multiplication algorithm with roughly equal input sizes.
///
/// Assumes `y.len() >= x.len()`.
fn karatsuba_mul<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    if y.len() <= KARATSUBA_CUTOFF {
        // Bottom-out to long division for small cases.
        long_mul(x, y)
    } else if x.len() < y.len() / 2 {
        karatsuba_uneven_mul(x, y)
    } else {
        // Do our 3 multiplications.
        let m = y.len() / 2;
        let (xl, xh) = karatsuba_split(x, m);
        let (yl, yh) = karatsuba_split(y, m);
        let sumx: T = add(xl, xh);
        let sumy: T = add(yl, yh);
        let z0: T = karatsuba_mul(xl, yl);
        let mut z1: T = karatsuba_mul(&sumx, &sumy);
        let z2: T = karatsuba_mul(xh, yh);
        // Properly scale z1, which is `z1 - z2 - zo`.
        isub(&mut z1, &z2);
        isub(&mut z1, &z0);

        // Create our result, which is equal to, in little-endian order:
        // [z0, z1 - z2 - z0, z2]
        //  z1 must be shifted m digits (2^(32m)) over.
        //  z2 must be shifted 2*m digits (2^(64m)) over.
        let mut result = T::default();
        let len = z0.len().max(m + z1.len()).max(2*m + z2.len());
        result.reserve_exact(len);
        result.extend_from_slice(&z0);
        iadd_impl(&mut result, &z1, m);
        iadd_impl(&mut result, &z2, 2*m);

        result
    }
}

/// Karatsuba multiplication algorithm where y is substantially larger than x.
///
/// Assumes `y.len() >= x.len()`.
fn karatsuba_uneven_mul<T>(x: &[Limb], mut y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut result = T::default();
    result.resize(x.len() + y.len(), 0);

    // This effectively is like grade-school multiplication between
    // two numbers, except we're using splits on `y`, and the intermediate
    // step is a Karatsuba multiplication.
    let mut start = 0;
    while y.len() != 0 {
        let m = x.len().min(y.len());
        let (yl, yh) = karatsuba_split(y, m);
        let prod: T = karatsuba_mul(x, yl);
        iadd_impl(&mut result, &prod, start);
        y = yh;
        start += m;
    }
    small::normalize(&mut result);

    result
}

perftools_inline!{
/// Forwarder to the proper Karatsuba algorithm.
fn karatsuba_mul_fwd<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    if x.len() < y.len() {
        karatsuba_mul(x, y)
    } else {
        karatsuba_mul(y, x)
    }
}}

perftools_inline!{
/// MulAssign bigint to bigint.
pub fn imul<T>(x: &mut T, y: &[Limb])
    where T: CloneableVecLike<Limb>
{
    if y.len() == 1 {
        small::imul(x, y[0]);
    } else {
        // We're not really in a condition where using Karatsuba
        // multiplication makes sense, so we're just going to use long
        // division. ~20% speedup compared to:
        //      *x = karatsuba_mul_fwd(x, y);
        *x = karatsuba_mul_fwd(x, y);
    }
}}

perftools_inline!{
/// Mul bigint to bigint.
pub fn mul<T>(x: &[Limb], y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    imul(&mut z, y);
    z
}}

// DIVISION

/// Constants for algorithm D.
const ALGORITHM_D_B: Wide = 1 << <Limb as Integer>::BITS;
const ALGORITHM_D_M: Wide = ALGORITHM_D_B - 1;

/// Calculate qhat (an estimate for the quotient).
///
/// This is step D3 in Algorithm D in "The Art of Computer Programming".
/// Assumes `x.len() > y.len()` and `y.len() >= 2`.
///
/// * `j`   - Current index on the iteration of the loop.
fn calculate_qhat(x: &[Limb], y: &[Limb], j: usize)
    -> Wide
{
    let n = y.len();

    // Estimate qhat of q[j]
    // Original Code:
    //  qhat = (x[j+n]*B + x[j+n-1])/y[n-1];
    //  rhat = (x[j+n]*B + x[j+n-1]) - qhat*y[n-1];
    let x_jn = as_wide(x[j+n]);
    let x_jn1 = as_wide(x[j+n-1]);
    let num = (x_jn << <Limb as Integer>::BITS) + x_jn1;
    let den = as_wide(y[n-1]);
    let mut qhat = num / den;
    let mut rhat = num - qhat * den;

    // Scale qhat and rhat
    // Original Code:
    //  again:
    //    if (qhat >= B || qhat*y[n-2] > B*rhat + x[j+n-2])
    //    { qhat = qhat - 1;
    //      rhat = rhat + y[n-1];
    //      if (rhat < B) goto again;
    //    }
    let x_jn2 = as_wide(x[j+n-2]);
    let y_n2 = as_wide(y[n-2]);
    let y_n1 = as_wide(y[n-1]);
    // This only happens when the leading bit of qhat is set.
    while qhat >= ALGORITHM_D_B || qhat * y_n2 > (rhat << <Limb as Integer>::BITS) + x_jn2 {
        qhat -= 1;
        rhat += y_n1;
        if rhat >= ALGORITHM_D_B {
            break;
        }
    }

    qhat
}

/// Multiply and subtract.
///
/// This is step D4 in Algorithm D in "The Art of Computer Programming",
/// and returns the remainder.
fn multiply_and_subtract<T>(x: &mut T, y: &T, qhat: Wide, j: usize)
    -> SignedWide
    where T: CloneableVecLike<Limb>
{
    let n = y.len();

    // Multiply and subtract
    // Original Code:
    //  k = 0;
    //  for (i = 0; i < n; i++) {
    //     p = qhat*y[i];
    //     t = x[i+j] - k - (p & 0xFFFFFFFFLL);
    //     x[i+j] = t;
    //     k = (p >> 32) - (t >> 32);
    //  }
    //  t = x[j+n] - k;
    //  x[j+n] = t;
    let mut k: SignedWide = 0;
    let mut t: SignedWide;
    for i in 0..n {
        let x_ij = as_signed_wide(x[i+j]);
        let y_i = as_wide(y[i]);
        let p = qhat * y_i;
        t = x_ij.wrapping_sub(k).wrapping_sub(as_signed_wide(p & ALGORITHM_D_M));
        x[i+j] = as_limb(t);
        k = as_signed_wide(p >> <Limb as Integer>::BITS) - (t >> <Limb as Integer>::BITS);
    }
    t = as_signed_wide(x[j+n]) - k;
    x[j+n] = as_limb(t);

    t
}

perftools_inline!{
/// Calculate the quotient from the estimate and the test.
///
/// This is a mix of step D5 and D6 in Algorithm D, so the algorithm
/// may work for single passes, without a quotient buffer.
fn test_quotient(qhat: Wide, t: SignedWide)
    -> Wide
{
    if t < 0 {
        qhat - 1
    } else {
        qhat
    }
}}

/// Add back.
///
/// This is step D6 in Algorithm D in "The Art of Computer Programming",
/// and adds back the remainder on the very unlikely scenario we overestimated
/// the quotient by 1. Subtract 1 from the quotient, and add back the
/// remainder.
///
/// This step should be specifically debugged, due to its low likelihood,
/// since the probability is ~2/b, where b in this case is 2^32 or 2^64.
fn add_back<T>(x: &mut T, y: &T, mut t: SignedWide, j: usize)
    where T: CloneableVecLike<Limb>
{
    let n = y.len();

    // Store quotient digits
    // If we subtracted too much, add back.
    // Original Code:
    //  q[j] = qhat;              // Store quotient digit.
    //  if (t < 0) {              // If we subtracted too
    //     q[j] = q[j] - 1;       // much, add back.
    //     k = 0;
    //     for (i = 0; i < n; i++) {
    //        t = (unsigned long long)x[i+j] + y[i] + k;
    //        x[i+j] = t;
    //        k = t >> 32;
    //     }
    //     x[j+n] = x[j+n] + k;
    //  }
    if t < 0 {
        let mut k: SignedWide = 0;
        for i in 0..n {
            t = as_signed_wide(as_wide(x[i+j]) + as_wide(y[i])) + k;
            x[i+j] = as_limb(t);
            k = t >> <Limb as Integer>::BITS;
        }
        let x_jn = as_signed_wide(x[j+n]) + k;
        x[j+n] = as_limb(x_jn);
    }
}

/// Calculate the remainder from the quotient.
///
/// This is step D8 in Algorithm D in "The Art of Computer Programming",
/// and "unnormalizes" to calculate the remainder from the quotient.
fn calculate_remainder<T>(x: &[Limb], y: &[Limb], s: usize)
    -> T
    where T: CloneableVecLike<Limb>
{
    // Calculate the remainder.
    // Original Code:
    //  for (i = 0; i < n-1; i++)
    //     r[i] = (x[i] >> s) | ((unsigned long long)x[i+1] << (32-s));
    //  r[n-1] = x[n-1] >> s;
    let n = y.len();
    let mut r = T::default();
    r.reserve_exact(n);
    let rs = <Limb as Integer>::BITS - s;
    for i in 0..n-1 {
        let xi = as_wide(x[i]) >> s;
        let xi1 = as_wide(x[i+1]) << rs;
        let ri = xi | xi1;
        r.push(as_limb(ri));
    }
    let x_n1 = x[n-1] >> s;
    r.push(as_limb(x_n1));

    r
}

/// Implementation of Knuth's Algorithm D, and return the quotient and remainder.
///
/// `x` is the dividend, and `y` is the divisor.
/// Assumes `x.len() > y.len()` and `y.len() >= 2`.
///
/// Based off the Hacker's Delight implementation of Knuth's Algorithm D
/// in "The Art of Computer Programming".
///     http://www.hackersdelight.org/hdcodetxt/divmnu64.c.txt
///
/// All Hacker's Delight code is public domain, so this routine shall
/// also be placed in the public domain. See:
///     https://www.hackersdelight.org/permissions.htm
fn algorithm_d_div<T>(x: &[Limb], y: &[Limb])
    -> (T, T)
    where T: CloneableVecLike<Limb>
{
    // Normalize the divisor so the leading-bit is set to 1.
    // x is the dividend, y is the divisor.
    // Need a leading zero on the numerator.
    let s = y.rindex(0).leading_zeros().as_usize();
    let m = x.len();
    let n = y.len();
    let mut xn: T = small::shl_bits(x, s);
    let yn: T = small::shl_bits(y, s);
    xn.push(0);

    // Store certain variables for the algorithm.
    let mut q = T::default();
    q.resize(m-n+1, 0);
    for j in (0..m-n+1).rev() {
        // Estimate the quotient
        let mut qhat = calculate_qhat(&xn, &yn, j);
        if qhat != 0 {
            let t = multiply_and_subtract(&mut xn, &yn, qhat, j);
            qhat = test_quotient(qhat, t);
            add_back(&mut xn, &yn, t, j);
        }
        q[j] = as_limb(qhat);
    }
    let mut r = calculate_remainder(&xn, &yn, s);

    // Normalize our results
    small::normalize(&mut q);
    small::normalize(&mut r);

    (q, r)
}

perftools_inline!{
/// DivAssign bigint to bigint.
pub fn idiv<T>(x: &mut T, y: &[Limb])
    -> T
    where T: CloneableVecLike<Limb>
{
    debug_assert!(y.len() != 0);

    if x.len() < y.len() {
        // Can optimize easily, since the quotient is 0,
        // and the remainder is x. Put before `y.len() == 1`, since
        // it optimizes when `x.len() == 0` nicely.
        let mut r = T::default();
        mem::swap(x, &mut r);
        r
    } else if y.len() == 1 {
        // Can optimize for division by a small value.
        let mut r = T::default();
        r.push(small::idiv(x, y[0]));
        r
    } else {
        let (q, r) = algorithm_d_div(x, y);
        *x = q;
        r
    }
}}

perftools_inline!{
/// Div bigint to bigint.
pub fn div<T>(x: &[Limb], y: &[Limb])
    -> (T, T)
    where T: CloneableVecLike<Limb>
{
    let mut z = T::default();
    z.extend_from_slice(x);
    let rem = idiv(&mut z, y);
    (z, rem)
}}

/// Emit a single digit for the quotient and store the remainder in-place.
///
/// An extremely efficient division algorithm for small quotients, requiring
/// you to know the full range of the quotient prior to use. For example,
/// with a quotient that can range from [0, 10), you must have 4 leading
/// zeros in the divisor, so we can use a single-limb division to get
/// an accurate estimate of the quotient. Since we always underestimate
/// the quotient, we can add 1 and then emit the digit.
///
/// Requires a non-normalized denominator, with at least [1-6] leading
/// zeros, depending on the base (for example, 1 for base2, 6 for base36).
///
/// Adapted from David M. Gay's dtoa, and therefore under an MIT license:
///     www.netlib.org/fp/dtoa.c
pub fn quorem<T>(x: &mut T, y: &T)
    -> Limb
    where T: CloneableVecLike<Limb>
{
    debug_assert!(y.len() > 0);
    let mask = as_wide(Limb::max_value());

    // Numerator is smaller the denominator, quotient always 0.
    let m = x.len();
    let n = y.len();
    if m < n {
        return 0;
    }

    // Calculate our initial estimate for q
    let mut q = x[m-1] / (y[n-1] + 1);

    // Need to calculate the remainder if we don't have a 0 quotient.
    if q != 0 {
        let mut borrow: Wide = 0;
        let mut carry: Wide = 0;
        for j in 0..m {
            let p = as_wide(y[j]) * as_wide(q) + carry;
            carry = p >> <Limb as Integer>::BITS;
            let t = as_wide(x[j]).wrapping_sub(p & mask).wrapping_sub(borrow);
            borrow = (t >> <Limb as Integer>::BITS) & 1;
            x[j] = as_limb(t);
        }
        small::normalize(x);
    }

    // Check if we under-estimated x.
    if greater_equal(x, y) {
        q += 1;
        let mut borrow: Wide = 0;
        let mut carry: Wide = 0;
        for j in 0..m {
            let p = as_wide(y[j]) + carry;
            carry = p >> <Limb as Integer>::BITS;
            let t = as_wide(x[j]).wrapping_sub(p & mask).wrapping_sub(borrow);
            borrow = (t >> <Limb as Integer>::BITS) & 1;
            x[j] = as_limb(t);
        }
        small::normalize(x);
    }

    q
}

}   // large

use crate::lib::cmp;
use super::small_powers::*;
use super::large_powers::*;

/// Generate the imul_pown wrappers.
macro_rules! imul_power {
    ($name:ident, $base:expr) => (
        perftools_inline!{
        /// Multiply by a power of $base.
        fn $name(&mut self, n: u32) {
            self.imul_power_impl($base, n)
        }}
    );
}

// TRAITS
// ------

/// Traits for shared operations for big integers.
///
/// None of these are implemented using normal traits, since these
/// are very expensive operations, and we want to deliberately
/// and explicitly use these functions.
pub(in crate::atof::algorithm) trait SharedOps: Clone + Sized + Default {
    /// Underlying storage type for a SmallOps.
    type StorageType: CloneableVecLike<Limb>;

    // DATA

    /// Get access to the underlying data
    fn data<'a>(&'a self) -> &'a Self::StorageType;

    /// Get access to the underlying data
    fn data_mut<'a>(&'a mut self) -> &'a mut Self::StorageType;

    // ZERO

    perftools_inline!{
    /// Check if the value is a normalized 0.
    fn is_zero(&self) -> bool {
        self.limb_length() == 0
    }}

    // RELATIVE OPERATIONS

    perftools_inline!{
    /// Compare self to y.
    fn compare(&self, y: &Self) -> cmp::Ordering {
        large::compare(self.data(), y.data())
    }}

    perftools_inline!{
    /// Check if self is greater than y.
    fn greater(&self, y: &Self) -> bool {
        large::greater(self.data(), y.data())
    }}

    perftools_inline!{
    /// Check if self is greater than or equal to y.
    fn greater_equal(&self, y: &Self) -> bool {
        large::greater_equal(self.data(), y.data())
    }}

    perftools_inline!{
    /// Check if self is less than y.
    fn less(&self, y: &Self) -> bool {
        large::less(self.data(), y.data())
    }}

    perftools_inline!{
    /// Check if self is less than or equal to y.
    fn less_equal(&self, y: &Self) -> bool {
        large::less_equal(self.data(), y.data())
    }}

    perftools_inline!{
    /// Check if self is equal to y.
    fn equal(&self, y: &Self) -> bool {
        large::equal(self.data(), y.data())
    }}

    // PROPERTIES

    perftools_inline!{
    /// Get the number of leading zero digits in the storage.
    /// Assumes the value is normalized.
    fn leading_zero_limbs(&self) -> usize {
        small::leading_zero_limbs(self.data())
    }}

    perftools_inline!{
    /// Get the number of trailing zero digits in the storage.
    /// Assumes the value is normalized.
    fn trailing_zero_limbs(&self) -> usize {
        small::trailing_zero_limbs(self.data())
    }}

    perftools_inline!{
    /// Get number of leading zero bits in the storage.
    /// Assumes the value is normalized.
    fn leading_zeros(&self) -> usize {
        small::leading_zeros(self.data())
    }}

    perftools_inline!{
    /// Get number of trailing zero bits in the storage.
    /// Assumes the value is normalized.
    fn trailing_zeros(&self) -> usize {
        small::trailing_zeros(self.data())
    }}

    perftools_inline!{
    /// Calculate the bit-length of the big-integer.
    /// Returns usize::max_value() if the value overflows,
    /// IE, if `self.data().len() > usize::max_value() / 8`.
    fn bit_length(&self) -> usize {
        small::bit_length(self.data())
    }}

    perftools_inline!{
    /// Calculate the digit-length of the big-integer.
    fn limb_length(&self) -> usize {
        small::limb_length(self.data())
    }}

    perftools_inline!{
    /// Get the high 16-bits from the bigint and if there are remaining bits.
    fn hi16(&self) -> (u16, bool) {
        self.data().as_slice().hi16()
    }}

    perftools_inline!{
    /// Get the high 32-bits from the bigint and if there are remaining bits.
    fn hi32(&self) -> (u32, bool) {
        self.data().as_slice().hi32()
    }}

    perftools_inline!{
    /// Get the high 64-bits from the bigint and if there are remaining bits.
    fn hi64(&self) -> (u64, bool) {
        self.data().as_slice().hi64()
    }}

    perftools_inline!{
    /// Get the high 128-bits from the bigint and if there are remaining bits.
    fn hi128(&self) -> (u128, bool) {
        self.data().as_slice().hi128()
    }}

    perftools_inline!{
    /// Pad the buffer with zeros to the least-significant bits.
    fn pad_zero_digits(&mut self, n: usize) -> usize {
        small::ishl_limbs(self.data_mut(), n);
        n
    }}

    // INTEGER CONVERSIONS

    // CREATION

    perftools_inline!{
    /// Create new big integer from u16.
    fn from_u16(x: u16) -> Self {
        let mut v = Self::default();
        let slc = split_u16(x);
        v.data_mut().extend_from_slice(&slc);
        v.normalize();
        v
    }}

    perftools_inline!{
    /// Create new big integer from u32.
    fn from_u32(x: u32) -> Self {
        let mut v = Self::default();
        let slc = split_u32(x);
        v.data_mut().extend_from_slice(&slc);
        v.normalize();
        v
    }}

    perftools_inline!{
    /// Create new big integer from u64.
    fn from_u64(x: u64) -> Self {
        let mut v = Self::default();
        let slc = split_u64(x);
        v.data_mut().extend_from_slice(&slc);
        v.normalize();
        v
    }}

    perftools_inline!{
    /// Create new big integer from u128.
    fn from_u128(x: u128) -> Self {
        let mut v = Self::default();
        let slc = split_u128(x);
        v.data_mut().extend_from_slice(&slc);
        v.normalize();
        v
    }}

    // NORMALIZE

    perftools_inline!{
    /// Normalize the integer, so any leading zero values are removed.
    fn normalize(&mut self) {
        small::normalize(self.data_mut());
    }}

    perftools_inline!{
    /// Get if the big integer is normalized.
    fn is_normalized(&self) -> bool {
        self.data().is_empty() || !self.data().rindex(0).is_zero()
    }}

    // SHIFTS

    perftools_inline!{
    /// Shift-left the entire buffer n bits.
    fn ishl(&mut self, n: usize) {
        small::ishl(self.data_mut(), n);
    }}

    perftools_inline!{
    /// Shift-left the entire buffer n bits.
    fn shl(&self, n: usize) -> Self {
        let mut x = self.clone();
        x.ishl(n);
        x
    }}

    perftools_inline!{
    /// Shift-right the entire buffer n bits.
    fn ishr(&mut self, n: usize, mut roundup: bool) {
        roundup &= small::ishr(self.data_mut(), n);

        // Round-up the least significant bit.
        if roundup {
            if self.data().is_empty() {
                self.data_mut().push(1);
            } else {
                self.data_mut()[0] += 1;
            }
        }
    }}

    perftools_inline!{
    /// Shift-right the entire buffer n bits.
    fn shr(&self, n: usize, roundup: bool) -> Self {
        let mut x = self.clone();
        x.ishr(n, roundup);
        x
    }}
}

/// Trait for small operations for arbitrary-precision numbers.
pub(in crate::atof::algorithm) trait SmallOps: SharedOps {
    // SMALL POWERS

    perftools_inline!{
    /// Get the small powers from the radix.
    fn small_powers(radix: u32) -> &'static [Limb] {
        get_small_powers(radix)
    }}

    perftools_inline!{
    /// Get the large powers from the radix.
    fn large_powers(radix: u32) -> &'static [&'static [Limb]] {
        get_large_powers(radix)
    }}

    // ADDITION

    perftools_inline!{
    /// AddAssign small integer.
    fn iadd_small(&mut self, y: Limb) {
        small::iadd(self.data_mut(), y);
    }}

    perftools_inline!{
    /// Add small integer to a copy of self.
    fn add_small(&self, y: Limb) -> Self {
        let mut x = self.clone();
        x.iadd_small(y);
        x
    }}

    // SUBTRACTION

    perftools_inline!{
    /// SubAssign small integer.
    /// Warning: Does no overflow checking, x must be >= y.
    fn isub_small(&mut self, y: Limb) {
        small::isub(self.data_mut(), y);
    }}

    perftools_inline!{
    /// Sub small integer to a copy of self.
    /// Warning: Does no overflow checking, x must be >= y.
    fn sub_small(&mut self, y: Limb) -> Self {
        let mut x = self.clone();
        x.isub_small(y);
        x
    }}

    // MULTIPLICATION

    perftools_inline!{
    /// MulAssign small integer.
    fn imul_small(&mut self, y: Limb) {
        small::imul(self.data_mut(), y);
    }}

    perftools_inline!{
    /// Mul small integer to a copy of self.
    fn mul_small(&self, y: Limb) -> Self {
        let mut x = self.clone();
        x.imul_small(y);
        x
    }}

    perftools_inline!{
    /// MulAssign by a power.
    fn imul_power_impl(&mut self, radix: u32, n: u32) {
        small::imul_power(self.data_mut(), radix, n);
    }}

    perftools_inline!{
    fn imul_power(&mut self, radix: u32, n: u32) {
        match radix {
            2  => self.imul_pow2(n),
            3  => self.imul_pow3(n),
            4  => self.imul_pow4(n),
            5  => self.imul_pow5(n),
            6  => self.imul_pow6(n),
            7  => self.imul_pow7(n),
            8  => self.imul_pow8(n),
            9  => self.imul_pow9(n),
            10 => self.imul_pow10(n),
            11 => self.imul_pow11(n),
            12 => self.imul_pow12(n),
            13 => self.imul_pow13(n),
            14 => self.imul_pow14(n),
            15 => self.imul_pow15(n),
            16 => self.imul_pow16(n),
            17 => self.imul_pow17(n),
            18 => self.imul_pow18(n),
            19 => self.imul_pow19(n),
            20 => self.imul_pow20(n),
            21 => self.imul_pow21(n),
            22 => self.imul_pow22(n),
            23 => self.imul_pow23(n),
            24 => self.imul_pow24(n),
            25 => self.imul_pow25(n),
            26 => self.imul_pow26(n),
            27 => self.imul_pow27(n),
            28 => self.imul_pow28(n),
            29 => self.imul_pow29(n),
            30 => self.imul_pow30(n),
            31 => self.imul_pow31(n),
            32 => self.imul_pow32(n),
            33 => self.imul_pow33(n),
            34 => self.imul_pow34(n),
            35 => self.imul_pow35(n),
            36 => self.imul_pow36(n),
            _  => unreachable!()
        }
    }}

    perftools_inline!{
    /// Multiply by a power of 2.
    fn imul_pow2(&mut self, n: u32) {
        self.ishl(n.as_usize())
    }}

    imul_power!(imul_pow3, 3);

    perftools_inline!{
    /// Multiply by a power of 4.
    fn imul_pow4(&mut self, n: u32) {
        self.imul_pow2(2*n);
    }}

    imul_power!(imul_pow5, 5);

    perftools_inline!{
    /// Multiply by a power of 6.
    fn imul_pow6(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow2(n);
    }}

    imul_power!(imul_pow7, 7);

    perftools_inline!{
    /// Multiply by a power of 8.
    fn imul_pow8(&mut self, n: u32) {
        self.imul_pow2(3*n);
    }}

    perftools_inline!{
    /// Multiply by a power of 9.
    fn imul_pow9(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow3(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 10.
    fn imul_pow10(&mut self, n: u32) {
        self.imul_pow5(n);
        self.imul_pow2(n);
    }}

    imul_power!(imul_pow11, 11);

    perftools_inline!{
    /// Multiply by a power of 12.
    fn imul_pow12(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow4(n);
    }}

    imul_power!(imul_pow13, 13);

    perftools_inline!{
    /// Multiply by a power of 14.
    fn imul_pow14(&mut self, n: u32) {
        self.imul_pow7(n);
        self.imul_pow2(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 15.
    fn imul_pow15(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow5(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 16.
    fn imul_pow16(&mut self, n: u32) {
        self.imul_pow2(4*n);
    }}

    imul_power!(imul_pow17, 17);

    perftools_inline!{
    /// Multiply by a power of 18.
    fn imul_pow18(&mut self, n: u32) {
        self.imul_pow9(n);
        self.imul_pow2(n);
    }}

    imul_power!(imul_pow19, 19);

    perftools_inline!{
    /// Multiply by a power of 20.
    fn imul_pow20(&mut self, n: u32) {
        self.imul_pow5(n);
        self.imul_pow4(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 21.
    fn imul_pow21(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow7(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 22.
    fn imul_pow22(&mut self, n: u32) {
        self.imul_pow11(n);
        self.imul_pow2(n);
    }}

    imul_power!(imul_pow23, 23);

    perftools_inline!{
    /// Multiply by a power of 24.
    fn imul_pow24(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow8(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 25.
    fn imul_pow25(&mut self, n: u32) {
        self.imul_pow5(n);
        self.imul_pow5(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 26.
    fn imul_pow26(&mut self, n: u32) {
        self.imul_pow13(n);
        self.imul_pow2(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 27.
    fn imul_pow27(&mut self, n: u32) {
        self.imul_pow9(n);
        self.imul_pow3(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 28.
    fn imul_pow28(&mut self, n: u32) {
        self.imul_pow7(n);
        self.imul_pow4(n);
    }}

    imul_power!(imul_pow29, 29);

    perftools_inline!{
    /// Multiply by a power of 30.
    fn imul_pow30(&mut self, n: u32) {
        self.imul_pow15(n);
        self.imul_pow2(n);
    }}

    imul_power!(imul_pow31, 31);

    perftools_inline!{
    /// Multiply by a power of 32.
    fn imul_pow32(&mut self, n: u32) {
        self.imul_pow2(5*n);
    }}

    perftools_inline!{
    /// Multiply by a power of 33.
    fn imul_pow33(&mut self, n: u32) {
        self.imul_pow3(n);
        self.imul_pow11(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 34.
    fn imul_pow34(&mut self, n: u32) {
        self.imul_pow17(n);
        self.imul_pow2(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 35.
    fn imul_pow35(&mut self, n: u32) {
        self.imul_pow5(n);
        self.imul_pow7(n);
    }}

    perftools_inline!{
    /// Multiply by a power of 36.
    fn imul_pow36(&mut self, n: u32) {
        self.imul_pow9(n);
        self.imul_pow4(n);
    }}

    // DIVISION

    perftools_inline!{
    /// DivAssign small integer, and return the remainder.
    fn idiv_small(&mut self, y: Limb) -> Limb {
        small::idiv(self.data_mut(), y)
    }}

    perftools_inline!{
    /// Div small integer to a copy of self, and return the remainder.
    fn div_small(&self, y: Limb) -> (Self, Limb) {
        let mut x = self.clone();
        let rem = x.idiv_small(y);
        (x, rem)
    }}

    // POWER

    perftools_inline!{
    /// Calculate self^n
    fn ipow(&mut self, n: Limb) {
        small::ipow(self.data_mut(), n);
    }}

    perftools_inline!{
    /// Calculate self^n
    fn pow(&self, n: Limb) -> Self {
        let mut x = self.clone();
        x.ipow(n);
        x
    }}
}

/// Trait for large operations for arbitrary-precision numbers.
pub(in crate::atof::algorithm) trait LargeOps: SmallOps {
    // ADDITION

    perftools_inline!{
    /// AddAssign large integer.
    fn iadd_large(&mut self, y: &Self) {
        large::iadd(self.data_mut(), y.data());
    }}

    perftools_inline!{
    /// Add large integer to a copy of self.
    fn add_large(&mut self, y: &Self) -> Self {
        let mut x = self.clone();
        x.iadd_large(y);
        x
    }}

    // SUBTRACTION

    perftools_inline!{
    /// SubAssign large integer.
    /// Warning: Does no overflow checking, x must be >= y.
    fn isub_large(&mut self, y: &Self) {
        large::isub(self.data_mut(), y.data());
    }}

    perftools_inline!{
    /// Sub large integer to a copy of self.
    /// Warning: Does no overflow checking, x must be >= y.
    fn sub_large(&mut self, y: &Self) -> Self {
        let mut x = self.clone();
        x.isub_large(y);
        x
    }}

    // MULTIPLICATION

    perftools_inline!{
    /// MulAssign large integer.
    fn imul_large(&mut self, y: &Self) {
        large::imul(self.data_mut(), y.data());
    }}

    perftools_inline!{
    /// Mul large integer to a copy of self.
    fn mul_large(&mut self, y: &Self) -> Self {
        let mut x = self.clone();
        x.imul_large(y);
        x
    }}

    // DIVISION

    perftools_inline!{
    /// DivAssign large integer and get remainder.
    fn idiv_large(&mut self, y: &Self) -> Self {
        let mut rem = Self::default();
        *rem.data_mut() = large::idiv(self.data_mut(), y.data());
        rem
    }}

    perftools_inline!{
    /// Div large integer to a copy of self and get quotient and remainder.
    fn div_large(&mut self, y: &Self) -> (Self, Self) {
        let mut x = self.clone();
        let rem = x.idiv_large(y);
        (x, rem)
    }}

    perftools_inline!{
    /// Calculate the fast quotient for a single limb-bit quotient.
    ///
    /// This requires a non-normalized divisor, where there at least
    /// `integral_binary_factor` 0 bits set, to ensure at maximum a single
    /// digit will be produced for a single base.
    ///
    /// Warning: This is not a general-purpose division algorithm,
    /// it is highly specialized for peeling off singular digits.
    fn quorem(&mut self, y: &Self) -> Limb {
        large::quorem(self.data_mut(), y.data())
    }}
}

#[cfg(test)]
mod tests {
    use crate::util::test::*;
    use super::*;

    #[derive(Clone, Default)]
    struct Bigint {
        data: DataType,
    }

    impl Bigint {
        #[inline]
        pub fn new() -> Bigint {
            Bigint { data: arrvec![] }
        }
    }

    impl SharedOps for Bigint {
        type StorageType = DataType;

        #[inline]
        fn data<'a>(&'a self) -> &'a Self::StorageType {
            &self.data
        }

        #[inline]
        fn data_mut<'a>(&'a mut self) -> &'a mut Self::StorageType {
            &mut self.data
        }
    }

    impl SmallOps for Bigint {
    }

    impl LargeOps for Bigint {
    }

    // SHARED OPS

    #[test]
    fn greater_test() {
        // Simple
        let x = Bigint { data: from_u32(&[1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.greater(&y));
        assert!(!x.greater(&x));
        assert!(y.greater(&x));

        // Check asymmetric
        let x = Bigint { data: from_u32(&[5, 1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(x.greater(&y));
        assert!(!x.greater(&x));
        assert!(!y.greater(&x));

        // Check when we use reverse ordering properly.
        let x = Bigint { data: from_u32(&[5, 1, 9]) };
        let y = Bigint { data: from_u32(&[6, 2, 8]) };
        assert!(x.greater(&y));
        assert!(!x.greater(&x));
        assert!(!y.greater(&x));

        // Complex scenario, check it properly uses reverse ordering.
        let x = Bigint { data: from_u32(&[0, 1, 9]) };
        let y = Bigint { data: from_u32(&[4294967295, 0, 9]) };
        assert!(x.greater(&y));
        assert!(!x.greater(&x));
        assert!(!y.greater(&x));
    }

    #[test]
    fn greater_equal_test() {
        // Simple
        let x = Bigint { data: from_u32(&[1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.greater_equal(&y));
        assert!(x.greater_equal(&x));
        assert!(y.greater_equal(&x));

        // Check asymmetric
        let x = Bigint { data: from_u32(&[5, 1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(x.greater_equal(&y));
        assert!(x.greater_equal(&x));
        assert!(!y.greater_equal(&x));

        // Check when we use reverse ordering properly.
        let x = Bigint { data: from_u32(&[5, 1, 9]) };
        let y = Bigint { data: from_u32(&[6, 2, 8]) };
        assert!(x.greater_equal(&y));
        assert!(x.greater_equal(&x));
        assert!(!y.greater_equal(&x));

        // Complex scenario, check it properly uses reverse ordering.
        let x = Bigint { data: from_u32(&[0, 1, 9]) };
        let y = Bigint { data: from_u32(&[4294967295, 0, 9]) };
        assert!(x.greater_equal(&y));
        assert!(x.greater_equal(&x));
        assert!(!y.greater_equal(&x));
    }

    #[test]
    fn equal_test() {
        // Simple
        let x = Bigint { data: from_u32(&[1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.equal(&y));
        assert!(x.equal(&x));
        assert!(!y.equal(&x));

        // Check asymmetric
        let x = Bigint { data: from_u32(&[5, 1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.equal(&y));
        assert!(x.equal(&x));
        assert!(!y.equal(&x));

        // Check when we use reverse ordering properly.
        let x = Bigint { data: from_u32(&[5, 1, 9]) };
        let y = Bigint { data: from_u32(&[6, 2, 8]) };
        assert!(!x.equal(&y));
        assert!(x.equal(&x));
        assert!(!y.equal(&x));

        // Complex scenario, check it properly uses reverse ordering.
        let x = Bigint { data: from_u32(&[0, 1, 9]) };
        let y = Bigint { data: from_u32(&[4294967295, 0, 9]) };
        assert!(!x.equal(&y));
        assert!(x.equal(&x));
        assert!(!y.equal(&x));
    }

    #[test]
    fn less_test() {
        // Simple
        let x = Bigint { data: from_u32(&[1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(x.less(&y));
        assert!(!x.less(&x));
        assert!(!y.less(&x));

        // Check asymmetric
        let x = Bigint { data: from_u32(&[5, 1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.less(&y));
        assert!(!x.less(&x));
        assert!(y.less(&x));

        // Check when we use reverse ordering properly.
        let x = Bigint { data: from_u32(&[5, 1, 9]) };
        let y = Bigint { data: from_u32(&[6, 2, 8]) };
        assert!(!x.less(&y));
        assert!(!x.less(&x));
        assert!(y.less(&x));

        // Complex scenario, check it properly uses reverse ordering.
        let x = Bigint { data: from_u32(&[0, 1, 9]) };
        let y = Bigint { data: from_u32(&[4294967295, 0, 9]) };
        assert!(!x.less(&y));
        assert!(!x.less(&x));
        assert!(y.less(&x));
    }

    #[test]
    fn less_equal_test() {
        // Simple
        let x = Bigint { data: from_u32(&[1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(x.less_equal(&y));
        assert!(x.less_equal(&x));
        assert!(!y.less_equal(&x));

        // Check asymmetric
        let x = Bigint { data: from_u32(&[5, 1]) };
        let y = Bigint { data: from_u32(&[2]) };
        assert!(!x.less_equal(&y));
        assert!(x.less_equal(&x));
        assert!(y.less_equal(&x));

        // Check when we use reverse ordering properly.
        let x = Bigint { data: from_u32(&[5, 1, 9]) };
        let y = Bigint { data: from_u32(&[6, 2, 8]) };
        assert!(!x.less_equal(&y));
        assert!(x.less_equal(&x));
        assert!(y.less_equal(&x));

        // Complex scenario, check it properly uses reverse ordering.
        let x = Bigint { data: from_u32(&[0, 1, 9]) };
        let y = Bigint { data: from_u32(&[4294967295, 0, 9]) };
        assert!(!x.less_equal(&y));
        assert!(x.less_equal(&x));
        assert!(y.less_equal(&x));
    }

    #[test]
    fn leading_zero_limbs_test() {
        assert_eq!(Bigint::new().leading_zero_limbs(), 0);

        assert_eq!(Bigint::from_u16(0xF).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u32(0xFF).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u64(0xFF00000000).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u128(0xFF000000000000000000000000).leading_zero_limbs(), 0);

        assert_eq!(Bigint::from_u16(0xF).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u32(0xF).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u64(0xF00000000).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u128(0xF000000000000000000000000).leading_zero_limbs(), 0);

        assert_eq!(Bigint::from_u16(0xF0).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u32(0xF0).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u64(0xF000000000).leading_zero_limbs(), 0);
        assert_eq!(Bigint::from_u128(0xF0000000000000000000000000).leading_zero_limbs(), 0);
    }

    #[test]
    fn trailing_zero_limbs_test() {
        assert_eq!(Bigint::new().trailing_zero_limbs(), 0);

        assert_eq!(Bigint { data: arrvec![0xFF] }.trailing_zero_limbs(), 0);
        assert_eq!(Bigint { data: arrvec![0, 0xFF000] }.trailing_zero_limbs(), 1);
        assert_eq!(Bigint { data: arrvec![0, 0, 0, 0xFF000] }.trailing_zero_limbs(), 3);
    }

    #[test]
    fn leading_zeros_test() {
        assert_eq!(Bigint::new().leading_zeros(), 0);

        assert_eq!(Bigint::from_u16(0xFF).leading_zeros(), <Limb as Integer>::BITS-8);
        assert_eq!(Bigint::from_u32(0xFF).leading_zeros(), <Limb as Integer>::BITS-8);
        assert_eq!(Bigint::from_u64(0xFF00000000).leading_zeros(), 24);
        assert_eq!(Bigint::from_u128(0xFF000000000000000000000000).leading_zeros(), 24);

        assert_eq!(Bigint::from_u16(0xF).leading_zeros(), <Limb as Integer>::BITS-4);
        assert_eq!(Bigint::from_u32(0xF).leading_zeros(), <Limb as Integer>::BITS-4);
        assert_eq!(Bigint::from_u64(0xF00000000).leading_zeros(), 28);
        assert_eq!(Bigint::from_u128(0xF000000000000000000000000).leading_zeros(), 28);

        assert_eq!(Bigint::from_u16(0xF0).leading_zeros(), <Limb as Integer>::BITS-8);
        assert_eq!(Bigint::from_u32(0xF0).leading_zeros(), <Limb as Integer>::BITS-8);
        assert_eq!(Bigint::from_u64(0xF000000000).leading_zeros(), 24);
        assert_eq!(Bigint::from_u128(0xF0000000000000000000000000).leading_zeros(), 24);
    }

    #[test]
    fn trailing_zeros_test() {
        assert_eq!(Bigint::new().trailing_zeros(), 0);

        assert_eq!(Bigint::from_u16(0xFF).trailing_zeros(), 0);
        assert_eq!(Bigint::from_u32(0xFF).trailing_zeros(), 0);
        assert_eq!(Bigint::from_u64(0xFF00000000).trailing_zeros(), 32);
        assert_eq!(Bigint::from_u128(0xFF000000000000000000000000).trailing_zeros(), 96);

        assert_eq!(Bigint::from_u16(0xF).trailing_zeros(), 0);
        assert_eq!(Bigint::from_u32(0xF).trailing_zeros(), 0);
        assert_eq!(Bigint::from_u64(0xF00000000).trailing_zeros(), 32);
        assert_eq!(Bigint::from_u128(0xF000000000000000000000000).trailing_zeros(), 96);

        assert_eq!(Bigint::from_u16(0xF0).trailing_zeros(), 4);
        assert_eq!(Bigint::from_u32(0xF0).trailing_zeros(), 4);
        assert_eq!(Bigint::from_u64(0xF000000000).trailing_zeros(), 36);
        assert_eq!(Bigint::from_u128(0xF0000000000000000000000000).trailing_zeros(), 100);
    }

    #[test]
    fn hi32_test() {
        assert_eq!(Bigint::from_u16(0xA).hi32(), (0xA0000000, false));
        assert_eq!(Bigint::from_u32(0xAB).hi32(), (0xAB000000, false));
        assert_eq!(Bigint::from_u64(0xAB00000000).hi32(), (0xAB000000, false));
        assert_eq!(Bigint::from_u64(0xA23456789A).hi32(), (0xA2345678, true));
    }

    #[test]
    fn hi64_test() {
        assert_eq!(Bigint::from_u16(0xA).hi64(), (0xA000000000000000, false));
        assert_eq!(Bigint::from_u32(0xAB).hi64(), (0xAB00000000000000, false));
        assert_eq!(Bigint::from_u64(0xAB00000000).hi64(), (0xAB00000000000000, false));
        assert_eq!(Bigint::from_u64(0xA23456789A).hi64(), (0xA23456789A000000, false));
        assert_eq!(Bigint::from_u128(0xABCDEF0123456789ABCDEF0123).hi64(), (0xABCDEF0123456789, true));
    }

    #[test]
    fn hi128_test() {
        assert_eq!(Bigint::from_u128(0xABCDEF0123456789ABCDEF0123).hi128(), (0xABCDEF0123456789ABCDEF0123000000, false));
        assert_eq!(Bigint::from_u128(0xABCDEF0123456789ABCDEF0123456789).hi128(), (0xABCDEF0123456789ABCDEF0123456789, false));
        assert_eq!(Bigint { data: from_u32(&[0x34567890, 0xBCDEF012, 0x3456789A, 0xBCDEF012, 0xA]) }.hi128(), (0xABCDEF0123456789ABCDEF0123456789, false));
        assert_eq!(Bigint { data: from_u32(&[0x34567891, 0xBCDEF012, 0x3456789A, 0xBCDEF012, 0xA]) }.hi128(), (0xABCDEF0123456789ABCDEF0123456789, true));
    }

    #[test]
    fn pad_zero_digits_test() {
        let mut x = Bigint { data: arrvec![0, 0, 0, 1] };
        x.pad_zero_digits(3);
        assert_eq!(x.data.as_slice(), &[0, 0, 0, 0, 0, 0, 1]);

        let mut x = Bigint { data: arrvec![1] };
        x.pad_zero_digits(1);
        assert_eq!(x.data.as_slice(), &[0, 1]);
    }

    #[test]
    fn shl_test() {
        // Pattern generated via `''.join(["1" +"0"*i for i in range(20)])`
        let mut big = Bigint { data: from_u32(&[0xD2210408]) };
        big.ishl(5);
        assert_eq!(big.data, from_u32(&[0x44208100, 0x1A]));
        big.ishl(32);
        assert_eq!(big.data, from_u32(&[0, 0x44208100, 0x1A]));
        big.ishl(27);
        assert_eq!(big.data, from_u32(&[0, 0, 0xD2210408]));

        // 96-bits of previous pattern
        let mut big = Bigint { data: from_u32(&[0x20020010, 0x8040100, 0xD2210408]) };
        big.ishl(5);
        assert_eq!(big.data, from_u32(&[0x400200, 0x802004, 0x44208101, 0x1A]));
        big.ishl(32);
        assert_eq!(big.data, from_u32(&[0, 0x400200, 0x802004, 0x44208101, 0x1A]));
        big.ishl(27);
        assert_eq!(big.data, from_u32(&[0, 0, 0x20020010, 0x8040100, 0xD2210408]));
    }

    #[test]
    fn shr_test() {
        // Simple case.
        let mut big = Bigint { data: from_u32(&[0xD2210408]) };
        big.ishr(5, false);
        assert_eq!(big.data, from_u32(&[0x6910820]));
        big.ishr(27, false);
        assert_eq!(big.data, from_u32(&[]));

        // Pattern generated via `''.join(["1" +"0"*i for i in range(20)])`
        let mut big = Bigint { data: from_u32(&[0x20020010, 0x8040100, 0xD2210408]) };
        big.ishr(5, false);
        assert_eq!(big.data, from_u32(&[0x1001000, 0x40402008, 0x6910820]));
        big.ishr(32, false);
        assert_eq!(big.data, from_u32(&[0x40402008, 0x6910820]));
        big.ishr(27, false);
        assert_eq!(big.data, from_u32(&[0xD2210408]));

        // Check no-roundup with halfway and even
        let mut big = Bigint { data: from_u32(&[0xD2210408]) };
        big.ishr(3, true);
        assert_eq!(big.data, from_u32(&[0x1A442081]));
        big.ishr(1, true);
        assert_eq!(big.data, from_u32(&[0xD221040]));

        let mut big = Bigint { data: from_u32(&[0xD2210408]) };
        big.ishr(4, true);
        assert_eq!(big.data, from_u32(&[0xD221040]));

        // Check roundup with halfway and odd
        let mut big = Bigint { data: from_u32(&[0xD2210438]) };
        big.ishr(3, true);
        assert_eq!(big.data, from_u32(&[0x1A442087]));
        big.ishr(1, true);
        assert_eq!(big.data, from_u32(&[0xD221044]));

        let mut big = Bigint { data: from_u32(&[0xD2210438]) };
        big.ishr(5, true);
        assert_eq!(big.data, from_u32(&[0x6910822]));
    }

    #[test]
    fn bit_length_test() {
        let x = Bigint { data: from_u32(&[0, 0, 0, 1]) };
        assert_eq!(x.bit_length(), 97);

        let x = Bigint { data: from_u32(&[0, 0, 0, 3]) };
        assert_eq!(x.bit_length(), 98);

        let x = Bigint { data: from_u32(&[1<<31]) };
        assert_eq!(x.bit_length(), 32);
    }

    // SMALL OPS

    #[test]
    fn iadd_small_test() {
        // Overflow check (single)
        // This should set all the internal data values to 0, the top
        // value to (1<<31), and the bottom value to (4>>1).
        // This is because the max_value + 1 leads to all 0s, we set the
        // topmost bit to 1.
        let mut x = Bigint { data: from_u32(&[4294967295]) };
        x.iadd_small(5);
        assert_eq!(x.data, from_u32(&[4, 1]));

        // No overflow, single value
        let mut x = Bigint { data: from_u32(&[5]) };
        x.iadd_small(7);
        assert_eq!(x.data, from_u32(&[12]));

        // Single carry, internal overflow
        let mut x = Bigint::from_u64(0x80000000FFFFFFFF);
        x.iadd_small(7);
        assert_eq!(x.data, from_u32(&[6, 0x80000001]));

        // Double carry, overflow
        let mut x = Bigint::from_u64(0xFFFFFFFFFFFFFFFF);
        x.iadd_small(7);
        assert_eq!(x.data, from_u32(&[6, 0, 1]));
    }

    #[test]
    fn isub_small_test() {
        // Overflow check (single)
        let mut x = Bigint { data: from_u32(&[4, 1]) };
        x.isub_small(5);
        assert_eq!(x.data, from_u32(&[4294967295]));

        // No overflow, single value
        let mut x = Bigint { data: from_u32(&[12]) };
        x.isub_small(7);
        assert_eq!(x.data, from_u32(&[5]));

        // Single carry, internal overflow
        let mut x = Bigint { data: from_u32(&[6, 0x80000001]) };
        x.isub_small(7);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFF, 0x80000000]));

        // Double carry, overflow
        let mut x = Bigint { data: from_u32(&[6, 0, 1]) };
        x.isub_small(7);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFF, 0xFFFFFFFF]));
    }

    #[test]
    fn imul_small_test() {
        // No overflow check, 1-int.
        let mut x = Bigint { data: from_u32(&[5]) };
        x.imul_small(7);
        assert_eq!(x.data, from_u32(&[35]));

        // No overflow check, 2-ints.
        let mut x = Bigint::from_u64(0x4000000040000);
        x.imul_small(5);
        assert_eq!(x.data, from_u32(&[0x00140000, 0x140000]));

        // Overflow, 1 carry.
        let mut x = Bigint { data: from_u32(&[0x33333334]) };
        x.imul_small(5);
        assert_eq!(x.data, from_u32(&[4, 1]));

        // Overflow, 1 carry, internal.
        let mut x = Bigint::from_u64(0x133333334);
        x.imul_small(5);
        assert_eq!(x.data, from_u32(&[4, 6]));

        // Overflow, 2 carries.
        let mut x = Bigint::from_u64(0x3333333333333334);
        x.imul_small(5);
        assert_eq!(x.data, from_u32(&[4, 0, 1]));
    }

    #[test]
    fn idiv_small_test() {
        let mut x = Bigint { data: from_u32(&[4]) };
        assert_eq!(x.idiv_small(7), 4);
        assert_eq!(x.data, from_u32(&[]));

        let mut x = Bigint { data: from_u32(&[3]) };
        assert_eq!(x.idiv_small(7), 3);
        assert_eq!(x.data, from_u32(&[]));

        // Check roundup, odd, halfway
        let mut x = Bigint { data: from_u32(&[15]) };
        assert_eq!(x.idiv_small(10), 5);
        assert_eq!(x.data, from_u32(&[1]));

        // Check 1 carry.
        let mut x = Bigint::from_u64(0x133333334);
        assert_eq!(x.idiv_small(5), 1);
        assert_eq!(x.data, from_u32(&[0x3D70A3D7]));

        // Check 2 carries.
        let mut x = Bigint::from_u64(0x3333333333333334);
        assert_eq!(x.idiv_small(5), 4);
        assert_eq!(x.data, from_u32(&[0xD70A3D70, 0xA3D70A3]));
    }

    #[test]
    fn ipow_test() {
        let x = Bigint { data: from_u32(&[5]) };
        assert_eq!(x.pow(2).data, from_u32(&[25]));
        assert_eq!(x.pow(15).data, from_u32(&[452807053, 7]));
        assert_eq!(x.pow(16).data, from_u32(&[2264035265, 35]));
        assert_eq!(x.pow(17).data, from_u32(&[2730241733, 177]));
        assert_eq!(x.pow(302).data, from_u32(&[2443090281, 2149694430, 2297493928, 1584384001, 1279504719, 1930002239, 3312868939, 3735173465, 3523274756, 2025818732, 1641675015, 2431239749, 4292780461, 3719612855, 4174476133, 3296847770, 2677357556, 638848153, 2198928114, 3285049351, 2159526706, 626302612]));
    }

    // LARGE OPS

    #[test]
    fn iadd_large_test() {
        // Overflow, both single values
        let mut x = Bigint { data: from_u32(&[4294967295]) };
        let y = Bigint { data: from_u32(&[5]) };
        x.iadd_large(&y);
        assert_eq!(x.data, from_u32(&[4, 1]));

        // No overflow, single value
        let mut x = Bigint { data: from_u32(&[5]) };
        let y = Bigint { data: from_u32(&[7]) };
        x.iadd_large(&y);
        assert_eq!(x.data, from_u32(&[12]));

        // Single carry, internal overflow
        let mut x = Bigint::from_u64(0x80000000FFFFFFFF);
        let y = Bigint { data: from_u32(&[7]) };
        x.iadd_large(&y);
        assert_eq!(x.data, from_u32(&[6, 0x80000001]));

        // 1st overflows, 2nd doesn't.
        let mut x = Bigint::from_u64(0x7FFFFFFFFFFFFFFF);
        let y = Bigint::from_u64(0x7FFFFFFFFFFFFFFF);
        x.iadd_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFE, 0xFFFFFFFF]));

        // Both overflow.
        let mut x = Bigint::from_u64(0x8FFFFFFFFFFFFFFF);
        let y = Bigint::from_u64(0x7FFFFFFFFFFFFFFF);
        x.iadd_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFE, 0x0FFFFFFF, 1]));
    }

    #[test]
    fn isub_large_test() {
        // Overflow, both single values
        let mut x = Bigint { data: from_u32(&[4, 1]) };
        let y = Bigint { data: from_u32(&[5]) };
        x.isub_large(&y);
        assert_eq!(x.data, from_u32(&[4294967295]));

        // No overflow, single value
        let mut x = Bigint { data: from_u32(&[12]) };
        let y = Bigint { data: from_u32(&[7]) };
        x.isub_large(&y);
        assert_eq!(x.data, from_u32(&[5]));

        // Single carry, internal overflow
        let mut x = Bigint { data: from_u32(&[6, 0x80000001]) };
        let y = Bigint { data: from_u32(&[7]) };
        x.isub_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFF, 0x80000000]));

        // Zeros out.
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFF, 0x7FFFFFFF]) };
        let y = Bigint { data: from_u32(&[0xFFFFFFFF, 0x7FFFFFFF]) };
        x.isub_large(&y);
        assert_eq!(x.data, from_u32(&[]));

        // 1st overflows, 2nd doesn't.
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFE, 0x80000000]) };
        let y = Bigint { data: from_u32(&[0xFFFFFFFF, 0x7FFFFFFF]) };
        x.isub_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFF]));
    }

    #[test]
    fn imul_large_test() {
        // Test by empty
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFF]) };
        let y = Bigint { data: from_u32(&[]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[]));

        // Simple case
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFF]) };
        let y = Bigint { data: from_u32(&[5]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFB, 0x4]));

        // Large u32, but still just as easy.
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFF]) };
        let y = Bigint { data: from_u32(&[0xFFFFFFFE]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[0x2, 0xFFFFFFFD]));

        // Let's multiply two large values together
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFE, 0x0FFFFFFF, 1]) };
        let y = Bigint { data: from_u32(&[0x99999999, 0x99999999, 0xCCCD9999, 0xCCCC]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[0xCCCCCCCE, 0x5CCCCCCC, 0x9997FFFF, 0x33319999, 0x999A7333, 0xD999]));
    }

    #[test]
    fn imul_karatsuba_mul_test() {
        // Test cases triggered to use `karatsuba_mul`.
        let mut x = Bigint { data: from_u32(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]) };
        let y = Bigint { data: from_u32(&[4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[4, 13, 28, 50, 80, 119, 168, 228, 300, 385, 484, 598, 728, 875, 1040, 1224, 1340, 1435, 1508, 1558, 1584, 1585, 1560, 1508, 1428, 1319, 1180, 1010, 808, 573, 304]));

        // Test cases to use karatsuba_uneven_mul
        let mut x = Bigint { data: from_u32(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]) };
        let y = Bigint { data: from_u32(&[4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37]) };
        x.imul_large(&y);
        assert_eq!(x.data, from_u32(&[4, 13, 28, 50, 80, 119, 168, 228, 300, 385, 484, 598, 728, 875, 1040, 1224, 1360, 1496, 1632, 1768, 1904, 2040, 2176, 2312, 2448, 2584, 2720, 2856, 2992, 3128, 3264, 3400, 3536, 3672, 3770, 3829, 3848, 3826, 3762, 3655, 3504, 3308, 3066, 2777, 2440, 2054, 1618, 1131, 592]));
    }

    #[test]
    fn idiv_large_test() {
        // Simple case.
        let mut x = Bigint { data: from_u32(&[0xFFFFFFFF]) };
        let y = Bigint { data: from_u32(&[5]) };
        let rem = x.idiv_large(&y);
        assert_eq!(x.data, from_u32(&[0x33333333]));
        assert_eq!(rem.data, from_u32(&[0]));

        // Two integer case
        let mut x = Bigint { data: from_u32(&[0x2, 0xFFFFFFFF]) };
        let y = Bigint { data: from_u32(&[0xFFFFFFFE]) };
        let rem = x.idiv_large(&y);
        assert_eq!(x.data, from_u32(&[1, 1]));
        assert_eq!(rem.data, from_u32(&[4]));

        // Larger large case
        let mut x = Bigint { data: from_u32(&[0xCCCCCCCF, 0x5CCCCCCC, 0x9997FFFF, 0x33319999, 0x999A7333, 0xD999]) };
        let y = Bigint { data: from_u32(&[0x99999999, 0x99999999, 0xCCCD9999, 0xCCCC]) };
        let rem = x.idiv_large(&y);
        assert_eq!(x.data, from_u32(&[0xFFFFFFFE, 0x0FFFFFFF, 1]));
        assert_eq!(rem.data, from_u32(&[1]));

        // Extremely large cases, examples from Karatsuba multiplication.
        let mut x = Bigint { data: from_u32(&[4, 13, 29, 50, 80, 119, 168, 228, 300, 385, 484, 598, 728, 875, 1040, 1224, 1340, 1435, 1508, 1558, 1584, 1585, 1560, 1508, 1428, 1319, 1180, 1010, 808, 573, 304]) };
        let y = Bigint { data: from_u32(&[4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]) };
        let rem = x.idiv_large(&y);
        assert_eq!(x.data, from_u32(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]));
        assert_eq!(rem.data, from_u32(&[0, 0, 1]));
    }

    #[test]
    fn quorem_test() {
        let mut x = Bigint::from_u128(42535295865117307932921825928971026432);
        let y = Bigint::from_u128(17218479456385750618067377696052635483);
        assert_eq!(x.quorem(&y), 2);
        assert_eq!(x.data, from_u32(&[1873752394, 3049207402, 3024501058, 102215382]));
    }
}
