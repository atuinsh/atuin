//! Unit tests for the `boxed` module.

use crate::prelude::*;

#[test]
#[allow(deprecated)]
fn api() {
	let boxed: Box<[u8]> = Box::new([0; 4]);
	let bb = BitBox::<LocalBits, _>::from_boxed_slice(boxed);
	assert_eq!(bb, bits![0; 32]);
	let boxed = bb.into_boxed_slice();
	assert_eq!(boxed[..], [0u8; 4][..]);

	let pinned = BitBox::pin(bits![0, 1, 0, 1]);
	let unpinned = BitBox::new(bits![0, 1, 0, 1]);
	assert_eq!(pinned.as_ref().get_ref(), unpinned[..]);

	let boxed = bitbox![0; 10];
	let bitptr = boxed.bitptr();
	let reboxed = unsafe { BitBox::from_raw(BitBox::into_raw(boxed)) };
	let bv = reboxed.into_bitvec();
	let bb = bv.into_boxed_bitslice();
	assert_eq!(bb.bitptr(), bitptr);

	let mut bb = 0b1001_0110u8.view_bits::<Msb0>()[2 .. 6]
		.to_bitvec()
		.into_boxed_bitslice();
	bb.set_uninitialized(false);
	assert_eq!(bb.as_slice(), &[0b0001_0100]);
	bb.set_uninitialized(true);
	assert_eq!(bb.as_slice(), &[0b1101_0111]);
	assert_eq!(bb, bits![0, 1, 0, 1]);
}
