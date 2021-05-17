#![allow(clippy::unreadable_literal)]

pub const STATE_LEN: usize = 5;

#[cfg(any(not(feature = "asm"), feature = "asm-aarch64"))]
pub const BLOCK_LEN: usize = 16;

#[cfg(any(not(feature = "asm"), feature = "asm-aarch64"))]
pub const K0: u32 = 0x5A827999u32;
#[cfg(any(not(feature = "asm"), feature = "asm-aarch64"))]
pub const K1: u32 = 0x6ED9EBA1u32;
#[cfg(any(not(feature = "asm"), feature = "asm-aarch64"))]
pub const K2: u32 = 0x8F1BBCDCu32;
#[cfg(any(not(feature = "asm"), feature = "asm-aarch64"))]
pub const K3: u32 = 0xCA62C1D6u32;

pub const H: [u32; STATE_LEN] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
