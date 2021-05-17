//! Trait implementations on `BitArray`.

use crate::{
	array::BitArray,
	index::BitIdx,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
	view::BitView,
};

use core::{
	borrow::{
		Borrow,
		BorrowMut,
	},
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
		UpperHex,
	},
	hash::{
		Hash,
		Hasher,
	},
};

#[cfg(not(tarpaulin_include))]
impl<O, V> Borrow<BitSlice<O, V::Store>> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn borrow(&self) -> &BitSlice<O, V::Store> {
		self.as_bitslice()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> BorrowMut<BitSlice<O, V::Store>> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn borrow_mut(&mut self) -> &mut BitSlice<O, V::Store> {
		self.as_mut_bitslice()
	}
}

impl<O, V> Eq for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Ord for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn cmp(&self, other: &Self) -> cmp::Ordering {
		self.as_bitslice().cmp(other.as_bitslice())
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V, T> PartialEq<BitArray<O, V>> for BitSlice<O, T>
where
	O: BitOrder,
	V: BitView + Sized,
	T: BitStore,
{
	#[inline]
	fn eq(&self, other: &BitArray<O, V>) -> bool {
		self == other.as_bitslice()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V, Rhs> PartialEq<Rhs> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
	Rhs: ?Sized,
	BitSlice<O, V::Store>: PartialEq<Rhs>,
{
	#[inline]
	fn eq(&self, other: &Rhs) -> bool {
		self.as_bitslice() == other
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V, T> PartialOrd<BitArray<O, V>> for BitSlice<O, T>
where
	O: BitOrder,
	V: BitView + Sized,
	T: BitStore,
{
	#[inline]
	fn partial_cmp(&self, other: &BitArray<O, V>) -> Option<cmp::Ordering> {
		self.partial_cmp(other.as_bitslice())
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V, Rhs> PartialOrd<Rhs> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
	Rhs: ?Sized,
	BitSlice<O, V::Store>: PartialOrd<Rhs>,
{
	#[inline]
	fn partial_cmp(&self, other: &Rhs) -> Option<cmp::Ordering> {
		self.as_bitslice().partial_cmp(other)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> AsRef<BitSlice<O, V::Store>> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn as_ref(&self) -> &BitSlice<O, V::Store> {
		self.as_bitslice()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> AsMut<BitSlice<O, V::Store>> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn as_mut(&mut self) -> &mut BitSlice<O, V::Store> {
		self.as_mut_bitslice()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> From<V> for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn from(data: V) -> Self {
		Self::new(data)
	}
}

impl<O1, O2, T, V> TryFrom<&'_ BitSlice<O2, T>> for BitArray<O1, V>
where
	O1: BitOrder,
	O2: BitOrder,
	T: BitStore,
	V: BitView + Sized,
{
	type Error = TryFromBitSliceError;

	#[inline]
	fn try_from(src: &BitSlice<O2, T>) -> Result<Self, Self::Error> {
		if src.len() != V::const_bits() {
			return Self::Error::err();
		}
		let mut out = Self::zeroed();
		out.clone_from_bitslice(src);
		Ok(out)
	}
}

impl<'a, O, V> TryFrom<&'a BitSlice<O, V::Store>> for &'a BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	type Error = TryFromBitSliceError;

	#[inline]
	fn try_from(src: &'a BitSlice<O, V::Store>) -> Result<Self, Self::Error> {
		let bitptr = src.bitptr();
		//  This pointer cast can only happen if the slice is exactly as long as
		//  the array, and is aligned to the front of the element.
		if src.len() != V::const_bits() || bitptr.head() != BitIdx::ZERO {
			return Self::Error::err();
		}
		Ok(unsafe { &*(bitptr.pointer().to_const() as *const BitArray<O, V>) })
	}
}

impl<'a, O, V> TryFrom<&'a mut BitSlice<O, V::Store>> for &'a mut BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	type Error = TryFromBitSliceError;

	#[inline]
	fn try_from(
		src: &'a mut BitSlice<O, V::Store>,
	) -> Result<Self, Self::Error> {
		let bitptr = src.bitptr();
		if src.len() != V::const_bits() || bitptr.head() != BitIdx::ZERO {
			return Self::Error::err();
		}
		Ok(unsafe { &mut *(bitptr.pointer().to_mut() as *mut BitArray<O, V>) })
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Default for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline(always)]
	fn default() -> Self {
		Self::zeroed()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Binary for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Binary::fmt(self.as_bitslice(), fmt)
	}
}

impl<O, V> Debug for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		if fmt.alternate() {
			self.bitptr().render(
				fmt,
				"Array",
				Some(core::any::type_name::<O>()),
				None,
			)?;
			fmt.write_str(" ")?;
		}
		Binary::fmt(self, fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Display for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Binary::fmt(self.as_bitslice(), fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> LowerHex for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		LowerHex::fmt(self.as_bitslice(), fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Octal for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Octal::fmt(self.as_bitslice(), fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> UpperHex for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		UpperHex::fmt(self.as_bitslice(), fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> Hash for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	#[inline]
	fn hash<H>(&self, hasher: &mut H)
	where H: Hasher {
		self.as_bitslice().hash(hasher)
	}
}

#[cfg(not(tarpaulin_include))]
impl<'a, O, V> IntoIterator for &'a BitArray<O, V>
where
	O: 'a + BitOrder,
	V: 'a + BitView + Sized,
{
	type IntoIter = <&'a BitSlice<O, V::Store> as IntoIterator>::IntoIter;
	type Item = <&'a BitSlice<O, V::Store> as IntoIterator>::Item;

	#[inline]
	fn into_iter(self) -> Self::IntoIter {
		self.as_bitslice().into_iter()
	}
}

#[cfg(not(tarpaulin_include))]
impl<'a, O, V> IntoIterator for &'a mut BitArray<O, V>
where
	O: 'a + BitOrder,
	V: 'a + BitView + Sized,
{
	type IntoIter = <&'a mut BitSlice<O, V::Store> as IntoIterator>::IntoIter;
	type Item = <&'a mut BitSlice<O, V::Store> as IntoIterator>::Item;

	#[inline]
	fn into_iter(self) -> Self::IntoIter {
		self.as_mut_bitslice().into_iter()
	}
}

impl<O, V> Unpin for BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
}

/// The error type returned when a conversion from a bitslice to a bitarray
/// fails.
#[derive(Clone, Copy, Debug)]
pub struct TryFromBitSliceError;

#[cfg(not(tarpaulin_include))]
impl TryFromBitSliceError {
	#[inline(always)]
	fn err<T>() -> Result<T, Self> {
		Err(Self)
	}
}

#[cfg(not(tarpaulin_include))]
impl Display for TryFromBitSliceError {
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		fmt.write_str("could not convert bitslice to bitarray")
	}
}

#[cfg(feature = "std")]
impl std::error::Error for TryFromBitSliceError {
}

#[cfg(test)]
mod tests {
	use crate::prelude::*;
	use core::convert::TryInto;

	#[test]
	fn convert() {
		let arr: BitArray<Lsb0, _> = 2u8.into();
		assert!(arr.any());

		let bits = bits![1; 128];
		let arr: BitArray<Msb0, [u16; 8]> = bits.try_into().unwrap();
		assert!(arr.all());

		let bits = bits![Lsb0, u32; 0; 64];
		let arr: &BitArray<Lsb0, [u32; 2]> = bits.try_into().unwrap();
		assert!(arr.not_any());

		let bits = bits![mut Msb0, u16; 0; 64];
		let arr: &mut BitArray<Msb0, [u16; 4]> = bits.try_into().unwrap();
		assert!(arr.not_any());

		let bits = bits![mut 0; 4];
		let bit_arr: Result<&BitArray<LocalBits, usize>, _> =
			(&*bits).try_into();
		assert!(bit_arr.is_err());
		let bit_arr: Result<&mut BitArray<LocalBits, usize>, _> =
			bits.try_into();
		assert!(bit_arr.is_err());
	}

	#[test]
	#[cfg(feature = "std")]
	fn format() {
		let render = format!("{:#?}", bitarr![Msb0, u8; 0, 1, 0, 0]);
		assert!(
			render.starts_with("BitArray<bitvec::order::Msb0, u8> {"),
			"{}",
			render
		);
		assert!(
			render.ends_with(
				"    head: 000,\n    bits: 8,\n} [\n    0b01000000,\n]"
			),
			"{}",
			render
		);
	}
}
