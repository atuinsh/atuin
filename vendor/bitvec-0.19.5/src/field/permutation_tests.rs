/*! Permutation testing.

This module runs battery tests on implementations of `BitField` to check that
they behave as expected.
!*/

use super::*;

#[cfg(not(miri))]
use crate::prelude::*;

/// Resizing always preserves the LSedge.
#[test]
fn check_resize() {
	assert_eq!(resize::<u8, u8>(0xA5u8), 0xA5u8);
	assert_eq!(resize::<u8, u16>(0xA5u8), 0xA5u16);
	assert_eq!(resize::<u8, u32>(0xA5u8), 0xA5u32);

	assert_eq!(resize::<u16, u8>(0x1234u16), 0x34u8);
	assert_eq!(resize::<u16, u16>(0x1234u16), 0x1234u16);
	assert_eq!(resize::<u16, u32>(0x1234u16), 0x1234u32);

	assert_eq!(resize::<u32, u8>(0x1234_5678u32), 0x78u8);
	assert_eq!(resize::<u32, u16>(0x1234_5678u32), 0x5678u16);
	assert_eq!(resize::<u32, u32>(0x1234_5678u32), 0x1234_5678u32);

	#[cfg(target_pointer_width = "64")]
	{
		assert_eq!(resize::<u8, u64>(0xA5u8), 0xA5u64);
		assert_eq!(resize::<u16, u64>(0x1234u16), 0x1234u64);
		assert_eq!(resize::<u32, u64>(0x1234_5678u32), 0x1234_5678u64);

		assert_eq!(resize::<u64, u8>(0x0123_4567_89AB_CDEFu64), 0xEFu8);
		assert_eq!(resize::<u64, u16>(0x0123_4567_89AB_CDEFu64), 0xCDEFu16);
		assert_eq!(resize::<u64, u32>(0x0123_4567_89AB_CDEFu64), 0x89AB_CDEFu32);
		assert_eq!(
			resize::<u64, u64>(0x0123_4567_89AB_CDEFu64),
			0x0123_4567_89AB_CDEFu64
		);
	}
}

#[test]
#[cfg(not(miri))]
fn l08() {
	let bits = bits![mut Lsb0, u8; 0; 32];

	for i in 0 .. 8 {
		for n in 0u8 ..= !0 {
			bits[i ..][.. 8].store_le::<u8>(n);
			assert_eq!(bits[i ..][.. 8].load_le::<u8>(), n);
		}
	}

	for i in 0 .. 16 {
		for n in 0u16 ..= !0 {
			bits[i ..][.. 16].store_le::<u16>(n);
			assert_eq!(bits[i ..][.. 16].load_le::<u16>(), n);
		}
	}
}

#[test]
#[cfg(not(miri))]
fn m08() {
	let bits = bits![mut Msb0, u8; 0; 32];

	for i in 0 .. 8 {
		for n in 0u8 ..= !0 {
			bits[i ..][.. 8].store_le::<u8>(n);
			assert_eq!(bits[i ..][.. 8].load_le::<u8>(), n);
		}
	}

	for i in 0 .. 16 {
		for n in 0u16 ..= !0 {
			bits[i ..][.. 16].store_le::<u16>(n);
			assert_eq!(bits[i ..][.. 16].load_le::<u16>(), n);
		}
	}
}

#[test]
#[cfg(not(miri))]
fn l16() {
	let bits = bits![mut Lsb0, u16; 0; 32];

	for i in 0 .. 8 {
		for n in 0u8 ..= !0 {
			bits[i ..][.. 8].store_le::<u8>(n);
			assert_eq!(bits[i ..][.. 8].load_le::<u8>(), n);
		}
	}

	for i in 0 .. 16 {
		for n in 0u16 ..= !0 {
			bits[i ..][.. 16].store_le::<u16>(n);
			assert_eq!(bits[i ..][.. 16].load_le::<u16>(), n);
		}
	}
}

#[test]
#[cfg(not(miri))]
fn m16() {
	let bits = bits![mut Msb0, u16; 0; 32];

	for i in 0 .. 8 {
		for n in 0u8 ..= !0 {
			bits[i ..][.. 8].store_le::<u8>(n);
			assert_eq!(bits[i ..][.. 8].load_le::<u8>(), n);
		}
	}

	for i in 0 .. 16 {
		for n in 0u16 ..= !0 {
			bits[i ..][.. 16].store_le::<u16>(n);
			assert_eq!(bits[i ..][.. 16].load_le::<u16>(), n);
		}
	}
}
