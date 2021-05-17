/*! I/O trait implementations.

The standard library defines byte-based I/O protocols that form the basis of
exchanging memory buffers with I/O controllers. As `BitSlice` is designed to be
used with I/O buffers, it makes sense for it to implement these protocols.

This module is a subset of the `field` module because it relies on the
`BitField` trait’s ability to map `BitSlice` to a value-storage region. The I/O
protocols `Read` and `Write` are strictly byte-based, and cannot be altered to
be bit-based. As such, they are only implemented on types with a `BitField`
implementation.

Calling `BitField` methods in a loop imposes a non-trivial, and irremovable,
per-loop overhead cost. Use of `bitvec` data structures directly, rather than
their underlying buffers, will have a performance penalty.
!*/

#![cfg(feature = "std")]

use crate::{
	field::BitField,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
	vec::BitVec,
};

use core::mem;

use std::io::{
	self,
	Read,
	Write,
};

/** Mirrors the implementation on `[u8]` (found [here]).

The implementation loads bytes out of the `&BitSlice` reference until exhaustion
of either the source `BitSlice` or destination `[u8]`. When `.read()` returns,
`self` will have been updated to no longer include the leading segment copied
out as bytes of `buf`.

[here]: https://doc.rust-lang.org/std/primitive.slice.html#impl-Read
**/
impl<'a, O, T> Read for &'a BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitField,
{
	#[inline]
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		let mut idx = 0;
		for (byte, slot) in self.chunks_exact(8).zip(buf.iter_mut()) {
			*slot = byte.load();
			idx += 1;
		}
		*self = unsafe { self.get_unchecked(idx * 8 ..) };
		Ok(idx)
	}
}

/** Mirrors the implementation on `[u8]` (found [here]).

The implementation copies bytes into the `&mut BitSlice` reference until
exhaustion of either the source `[u8]` or destination `BitSlice`. When
`.write()` returns, `self` will have been updated to no longer include the
leading segment containing bytes copied in from `buf`.

[here]: https://doc.rust-lang.org/std/primitive.slice.html#impl-Write
**/
impl<'a, O, T> Write for &'a mut BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T::Alias>: BitField,
{
	#[inline]
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		let mut idx = 0;
		for (slot, byte) in self.chunks_exact_mut(8).zip(buf.iter().copied()) {
			slot.store(byte);
			idx += 1;
		}
		*self = unsafe { mem::take(self).get_unchecked_mut(idx * 8 ..) };
		Ok(idx)
	}

	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

/** Mirrors the implementation on `Vec<u8>` (found [here]).

The implementation copies bytes from `buf` into the tail end of `self`. The
performance characteristics of this operation are dependent on the type
parameters of the `BitVec`, and the position of its tail.

[here]: https://doc.rust-lang.org/std/vec/struct.Vec.html#impl-Write
**/
impl<O, T> Write for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T::Alias>: BitField,
{
	#[inline]
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		let len = self.len();
		self.resize(len + buf.len() * 8, false);
		unsafe { self.get_unchecked_mut(len ..) }.write(buf)
	}

	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::prelude::*;

	#[test]
	fn read_bits() {
		let data = [0x136Cu16, 0x8C63];
		let mut bits = &data.view_bits::<Msb0>()[4 ..];
		assert_eq!(bits.len(), 28);

		let mut transfer = [0u8; 4];
		let last_ptr = &mut transfer[3] as *mut _;
		let mut transfer_handle = &mut transfer[..];

		assert_eq!(io::copy(&mut bits, &mut transfer_handle).unwrap(), 3);

		//  Once a bitslice cannot produce a byte, it stops `Read`ing
		assert_eq!(bits, data.view_bits::<Msb0>()[28 ..]);
		//  So the destination slice does not fill up.
		assert_eq!(transfer_handle.as_mut_ptr() as *mut _, last_ptr);

		if cfg!(target_endian = "little") {
			assert_eq!(transfer[.. 3], [0x36, 0x8C, 0xC6][..]);
			/* note the backwards nibbles here! ^^

			When crossing element boundaries, `.load_le()` assumes that the
			lesser memory address is less significant, and the greater memory
			address is more significant. The last nibble of the first element
			is therefore assumed to be numerically less significant than the
			first nibble of the second word.

			If this behavior surprises users, then an iterative copy may be more
			appropriate than a `BitField`-based load/store behavior. A bitwise
			crawl is slower, which is why `BitField` was chosen as the
			implementation. But “quickly wrong” is worse than “slowly right”.
			*/
		}
	}

	#[test]
	fn write_bits() {
		let mut bv = bitvec![Msb0, usize; 0, 0, 0, 0];
		assert_eq!(
			3,
			io::copy(&mut &[0xC3u8, 0xF0, 0x69][..], &mut bv).unwrap()
		);

		assert_eq!(bv, bits![
			0, 0, 0, 0, // byte 0
			1, 1, 0, 0, 0, 0, 1, 1, // byte 1
			1, 1, 1, 1, 0, 0, 0, 0, // byte 2
			0, 1, 1, 0, 1, 0, 0, 1,
		]);
	}
}
