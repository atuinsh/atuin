//! Trait implementations for `BitSlice`

use crate::{
	domain::Domain,
	index::BitRegister,
	mem::BitMemory,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
	view::BitView,
};

use core::{
	any,
	cmp,
	convert::TryFrom,
	fmt::{
		self,
		Binary,
		Debug,
		Display,
		Formatter,
		LowerHex,
		Octal,
		Pointer,
		UpperHex,
	},
	hash::{
		Hash,
		Hasher,
	},
	str,
};

use tap::pipe::Pipe;

#[cfg(feature = "alloc")]
use crate::vec::BitVec;

#[cfg(feature = "alloc")]
use alloc::borrow::ToOwned;

impl<O, T> Eq for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
}

impl<O, T> Ord for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn cmp(&self, rhs: &Self) -> cmp::Ordering {
		self.partial_cmp(rhs)
			.expect("BitSlice has a total ordering")
	}
}

/** Tests if two `BitSlice`s are semantically — not bitwise — equal.

It is valid to compare slices of different ordering or memory types.

The equality condition requires that they have the same length and that at each
index, the two slices have the same bit value.
**/
impl<O1, O2, T1, T2> PartialEq<BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	fn eq(&self, rhs: &BitSlice<O2, T2>) -> bool {
		self.len() == rhs.len()
			&& self.iter().zip(rhs.iter()).all(|(l, r)| l == r)
	}
}

//  ref-to-val equality

#[cfg(not(tarpaulin_include))]
impl<O1, O2, T1, T2> PartialEq<BitSlice<O2, T2>> for &BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn eq(&self, rhs: &BitSlice<O2, T2>) -> bool {
		**self == rhs
	}
}

#[cfg(not(tarpaulin_include))]
impl<O1, O2, T1, T2> PartialEq<BitSlice<O2, T2>> for &mut BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn eq(&self, rhs: &BitSlice<O2, T2>) -> bool {
		**self == rhs
	}
}

//  val-to-ref equality

#[cfg(not(tarpaulin_include))]
impl<O1, O2, T1, T2> PartialEq<&BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn eq(&self, rhs: &&BitSlice<O2, T2>) -> bool {
		*self == **rhs
	}
}

#[cfg(not(tarpaulin_include))]
impl<O1, O2, T1, T2> PartialEq<&mut BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn eq(&self, rhs: &&mut BitSlice<O2, T2>) -> bool {
		*self == **rhs
	}
}

/** Compares two `BitSlice`s by semantic — not bitwise — ordering.

The comparison sorts by testing at each index if one slice has a high bit where
the other has a low. At the first index where the slices differ, the slice with
the high bit is greater. If the slices are equal until at least one terminates,
then they are compared by length.
**/
impl<O1, O2, T1, T2> PartialOrd<BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	fn partial_cmp(&self, rhs: &BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		for (l, r) in self.iter().zip(rhs.iter()) {
			match (l, r) {
				(true, false) => return Some(cmp::Ordering::Greater),
				(false, true) => return Some(cmp::Ordering::Less),
				_ => continue,
			}
		}
		self.len().partial_cmp(&rhs.len())
	}
}

//  ref-to-val ordering

impl<O1, O2, T1, T2> PartialOrd<BitSlice<O2, T2>> for &BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(*self).partial_cmp(rhs)
	}
}

impl<O1, O2, T1, T2> PartialOrd<BitSlice<O2, T2>> for &mut BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(**self).partial_cmp(rhs)
	}
}

//  val-to-ref ordering

impl<O1, O2, T1, T2> PartialOrd<&BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &&BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(*self).partial_cmp(&**rhs)
	}
}

impl<O1, O2, T1, T2> PartialOrd<&mut BitSlice<O2, T2>> for BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &&mut BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(*self).partial_cmp(&**rhs)
	}
}

//  &mut-to-& ordering

impl<O1, O2, T1, T2> PartialOrd<&mut BitSlice<O2, T2>> for &BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &&mut BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(**self).partial_cmp(&**rhs)
	}
}

impl<O1, O2, T1, T2> PartialOrd<&BitSlice<O2, T2>> for &mut BitSlice<O1, T1>
where
	O1: BitOrder,
	O2: BitOrder,
	T1: BitStore,
	T2: BitStore,
{
	#[inline]
	fn partial_cmp(&self, rhs: &&BitSlice<O2, T2>) -> Option<cmp::Ordering> {
		(**self).partial_cmp(&**rhs)
	}
}

#[cfg(not(tarpaulin_include))]
impl<'a, O, T> TryFrom<&'a [T]> for &'a BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore + BitRegister,
{
	type Error = &'a [T];

	#[inline]
	fn try_from(slice: &'a [T]) -> Result<Self, Self::Error> {
		BitSlice::from_slice(slice).ok_or(slice)
	}
}

impl<O, T> Default for &BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn default() -> Self {
		BitSlice::empty()
	}
}

impl<O, T> Default for &mut BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn default() -> Self {
		BitSlice::empty_mut()
	}
}

impl<O, T> Debug for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		if fmt.alternate() {
			Pointer::fmt(self, fmt)?;
			fmt.write_str(" ")?;
		}
		Binary::fmt(self, fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> Display for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Binary::fmt(self, fmt)
	}
}

/** Renders a `BitSlice` handle as its pointer representation.

This does not enable `{:p}` in a format string, as there is a blanket `Pointer`
implementation for all references, and unsized types cannot format by
themselves. It is only reachable by forwarding from another format marker, such
as `Debug`.
**/
impl<O, T> Pointer for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		self.bitptr()
			.render(fmt, "Slice", Some(any::type_name::<O>()), None)
	}
}

/// Constructs numeric formatting implementations.
macro_rules! fmt {
	($trait:ident, $base:expr, $pfx:expr, $blksz:expr) => {
		/// Render the contents of a `BitSlice` in a numeric format.
		///
		/// These implementations render the bits of memory contained in a
		/// `BitSlice` as one of the three numeric bases that the Rust format
		/// system supports:
		///
		/// - `Binary` renders each bit individually as `0` or `1`,
		/// - `Octal` renders clusters of three bits as the numbers `0` through
		///   `7`,
		/// - and `UpperHex` and `LowerHex` render clusters of four bits as the
		///   numbers `0` through `9` and `A` through `F`.
		///
		/// The formatters produce a “word” for each element `T` of memory. The
		/// chunked formats (octal and hexadecimal) operate somewhat peculiarly:
		/// they show the semantic value of the memory, as interpreted by the
		/// ordering parameter’s implementation rather than the raw value of
		/// memory you might observe with a debugger. In order to ease the
		/// process of expanding numbers back into bits, each digit is grouped to
		/// the right edge of the memory element. So, for example, the byte
		/// `0xFF` would be rendered in as `0o377` rather than `0o773`.
		///
		/// Rendered words are chunked by memory elements, rather than by as
		/// clean as possible a number of digits, in order to aid visualization
		/// of the slice’s place in memory.
		impl<O, T> $trait for BitSlice<O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				/// Renders an accumulated text buffer as UTF-8.
				struct Seq<'a>(&'a [u8]);
				impl Debug for Seq<'_> {
					fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
						fmt.write_str(unsafe {
							str::from_utf8_unchecked(self.0)
						})
					}
				}
				//  If the alternate flag is set, include the radix prefix.
				let start = if fmt.alternate() { 0 } else { 2 };
				//  Create a list format accumulator.
				let mut dbg = fmt.debug_list();
				/* Create a static buffer of the maximum number of UTF-8 bytes
				needed to render a `usize` in the selected radix. Rust does not
				yet grant access to trait constants for use in constant
				expressions within generics.
				*/
				const W: usize = <usize as BitMemory>::BITS as usize / $blksz;
				let mut w: [u8; W + 2] = [b'0'; W + 2];
				//  Write the prefix symbol into the buffer.
				w[1] = $pfx;
				//  This closure does the main work of rendering a bit slice as
				//  text. It will be called on each memory element of the slice
				//  undergoing formatting.
				let mut writer = |bits: &BitSlice<O, T::Mem>| {
					//  Set the end index of the format buffer.
					let mut end = 2;
					/* Taking `rchunks` clusters the bits to the right edge, so
					that any remainder is in the left-most (first-rendered)
					digit, in the same manner as how English clusters digits in
					ordinary writing.

					Since `rchunks` takes from the back, it must be reversed in
					order to traverse from front to back. The enumeration
					provides the offset from the buffer start for writing the
					computed digit into the format buffer.
					*/
					for (index, chunk) in bits.rchunks($blksz).rev().enumerate()
					{
						//  Accumulate an Lsb0 representation of the slice
						//  contents.
						let mut val = 0u8;
						for bit in chunk {
							val <<= 1;
							val |= *bit as u8;
						}
						//  Translate the accumulator into ASCII hexadecimal
						//  glyphs, and write the glyph into the format buffer.
						w[2 + index] = match val {
							v @ 0 ..= 9 => b'0' + v,
							v @ 10 ..= 16 => $base + (v - 10),
							_ => unsafe { core::hint::unreachable_unchecked() },
						};
						end += 1;
					}
					//  View the format buffer as UTF-8 and write it into the
					//  main formatter.
					dbg.entry(&Seq(&w[start .. end]));
				};
				//  Break the source `BitSlice` into its element-wise components.
				match self.domain() {
					Domain::Enclave { head, elem, tail } => {
						//  Load a copy of `*elem` into the stack,
						let tmp = elem.load_value();
						//  View it as a `BitSlice` over the whole element,
						// narrow it to the live range, and render it.
						let bits = tmp.view_bits::<O>();
						unsafe {
							bits.get_unchecked(
								head.value() as usize .. tail.value() as usize,
							)
						}
						.pipe(writer);
					},
					//  Same process as above, but with different truncations.
					Domain::Region { head, body, tail } => {
						if let Some((head, elem)) = head {
							let tmp = elem.load_value();
							let bits = tmp.view_bits::<O>();
							unsafe {
								bits.get_unchecked(head.value() as usize ..)
							}
							.pipe(&mut writer);
						}
						for elem in body.iter() {
							elem.pipe(BitSlice::<O, T::Mem>::from_element)
								.pipe(&mut writer);
						}
						if let Some((elem, tail)) = tail {
							let tmp = elem.load_value();
							let bits = tmp.view_bits::<O>();
							unsafe {
								bits.get_unchecked(.. tail.value() as usize)
							}
							.pipe(&mut writer);
						}
					},
				}
				dbg.finish()
			}
		}
	};
}

fmt!(Binary, b'0', b'b', 1);
fmt!(Octal, b'0', b'o', 3);
fmt!(LowerHex, b'a', b'x', 4);
fmt!(UpperHex, b'A', b'x', 4);

/// Writes the contents of the `BitSlice`, in semantic bit order, into a hasher.
#[cfg(not(tarpaulin_include))]
impl<O, T> Hash for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn hash<H>(&self, hasher: &mut H)
	where H: Hasher {
		for bit in self {
			hasher.write_u8(*bit as u8);
		}
	}
}

unsafe impl<O, T> Send for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Threadsafe: Send,
{
}

unsafe impl<O, T> Sync for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Threadsafe: Sync,
{
}

#[cfg(feature = "alloc")]
impl<O, T> ToOwned for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	type Owned = BitVec<O, T>;

	#[inline]
	fn to_owned(&self) -> Self::Owned {
		BitVec::from_bitslice(self)
	}
}

#[cfg(test)]
mod tests {
	use crate::prelude::*;

	#[test]
	fn cmp() {
		let data = 0x45u8;
		let bits = data.view_bits::<Msb0>();
		let a = &bits[.. 3]; // 010
		let b = &bits[.. 4]; // 0100
		let c = &bits[.. 5]; // 01000
		let d = &bits[4 ..]; // 0101

		assert!(a < b); // by length
		assert!(b < c); // by length
		assert!(c < d); // by different bit
	}
}

#[cfg(all(test, feature = "alloc"))]
mod format {
	use crate::prelude::*;

	//  The `format!` macro is not in the `alloc` prelude.
	#[cfg(not(feature = "std"))]
	use alloc::format;

	#[test]
	fn binary() {
		let data = [0u8, 0x0F, !0];
		let bits = data.view_bits::<Msb0>();

		assert_eq!(format!("{:b}", &bits[.. 0]), "[]");
		assert_eq!(format!("{:#b}", &bits[.. 0]), "[]");

		assert_eq!(format!("{:b}", &bits[9 .. 15]), "[000111]");
		assert_eq!(
			format!("{:#b}", &bits[9 .. 15]),
			"[
    0b000111,
]"
		);

		assert_eq!(format!("{:b}", &bits[4 .. 20]), "[0000, 00001111, 1111]");
		assert_eq!(
			format!("{:#b}", &bits[4 .. 20]),
			"[
    0b0000,
    0b00001111,
    0b1111,
]"
		);

		assert_eq!(format!("{:b}", &bits[4 ..]), "[0000, 00001111, 11111111]");
		assert_eq!(
			format!("{:#b}", &bits[4 ..]),
			"[
    0b0000,
    0b00001111,
    0b11111111,
]"
		);

		assert_eq!(format!("{:b}", &bits[.. 20]), "[00000000, 00001111, 1111]");
		assert_eq!(
			format!("{:#b}", &bits[.. 20]),
			"[
    0b00000000,
    0b00001111,
    0b1111,
]"
		);

		assert_eq!(format!("{:b}", bits), "[00000000, 00001111, 11111111]");
		assert_eq!(
			format!("{:#b}", bits),
			"[
    0b00000000,
    0b00001111,
    0b11111111,
]"
		);
	}

	#[test]
	fn octal() {
		let data = [0u8, 0x0F, !0];
		let bits = data.view_bits::<Msb0>();

		assert_eq!(format!("{:o}", &bits[.. 0]), "[]");
		assert_eq!(format!("{:#o}", &bits[.. 0]), "[]");

		assert_eq!(format!("{:o}", &bits[9 .. 15]), "[07]");
		assert_eq!(
			format!("{:#o}", &bits[9 .. 15]),
			"[
    0o07,
]"
		);

		//  …0_000 00_001_111 1_111…
		assert_eq!(format!("{:o}", &bits[4 .. 20]), "[00, 017, 17]");
		assert_eq!(
			format!("{:#o}", &bits[4 .. 20]),
			"[
    0o00,
    0o017,
    0o17,
]"
		);

		assert_eq!(format!("{:o}", &bits[4 ..]), "[00, 017, 377]");
		assert_eq!(
			format!("{:#o}", &bits[4 ..]),
			"[
    0o00,
    0o017,
    0o377,
]"
		);

		assert_eq!(format!("{:o}", &bits[.. 20]), "[000, 017, 17]");
		assert_eq!(
			format!("{:#o}", &bits[.. 20]),
			"[
    0o000,
    0o017,
    0o17,
]"
		);

		assert_eq!(format!("{:o}", bits), "[000, 017, 377]");
		assert_eq!(
			format!("{:#o}", bits),
			"[
    0o000,
    0o017,
    0o377,
]"
		);
	}

	#[test]
	fn hex_lower() {
		let data = [0u8, 0x0F, !0];
		let bits = data.view_bits::<Msb0>();

		assert_eq!(format!("{:x}", &bits[.. 0]), "[]");
		assert_eq!(format!("{:#x}", &bits[.. 0]), "[]");

		//  …00_0111 …
		assert_eq!(format!("{:x}", &bits[9 .. 15]), "[07]");
		assert_eq!(
			format!("{:#x}", &bits[9 .. 15]),
			"[
    0x07,
]"
		);

		//  …0000 00001111 1111…
		assert_eq!(format!("{:x}", &bits[4 .. 20]), "[0, 0f, f]");
		assert_eq!(
			format!("{:#x}", &bits[4 .. 20]),
			"[
    0x0,
    0x0f,
    0xf,
]"
		);

		assert_eq!(format!("{:x}", &bits[4 ..]), "[0, 0f, ff]");
		assert_eq!(
			format!("{:#x}", &bits[4 ..]),
			"[
    0x0,
    0x0f,
    0xff,
]"
		);

		assert_eq!(format!("{:x}", &bits[.. 20]), "[00, 0f, f]");
		assert_eq!(
			format!("{:#x}", &bits[.. 20]),
			"[
    0x00,
    0x0f,
    0xf,
]"
		);

		assert_eq!(format!("{:x}", bits), "[00, 0f, ff]");
		assert_eq!(
			format!("{:#x}", bits),
			"[
    0x00,
    0x0f,
    0xff,
]"
		);
	}

	#[test]
	fn hex_upper() {
		let data = [0u8, 0x0F, !0];
		let bits = data.view_bits::<Msb0>();

		assert_eq!(format!("{:X}", &bits[.. 0]), "[]");
		assert_eq!(format!("{:#X}", &bits[.. 0]), "[]");

		assert_eq!(format!("{:X}", &bits[9 .. 15]), "[07]");
		assert_eq!(
			format!("{:#X}", &bits[9 .. 15]),
			"[
    0x07,
]"
		);

		assert_eq!(format!("{:X}", &bits[4 .. 20]), "[0, 0F, F]");
		assert_eq!(
			format!("{:#X}", &bits[4 .. 20]),
			"[
    0x0,
    0x0F,
    0xF,
]"
		);

		assert_eq!(format!("{:X}", &bits[4 ..]), "[0, 0F, FF]");
		assert_eq!(
			format!("{:#X}", &bits[4 ..]),
			"[
    0x0,
    0x0F,
    0xFF,
]"
		);

		assert_eq!(format!("{:X}", &bits[.. 20]), "[00, 0F, F]");
		assert_eq!(
			format!("{:#X}", &bits[.. 20]),
			"[
    0x00,
    0x0F,
    0xF,
]"
		);

		assert_eq!(format!("{:X}", bits), "[00, 0F, FF]");
		assert_eq!(
			format!("{:#X}", bits),
			"[
    0x00,
    0x0F,
    0xFF,
]"
		);
	}
}
