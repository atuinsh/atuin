//! Operator trait implementations.

use crate::{
	boxed::BitBox,
	devel as dvl,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
};

use core::{
	mem::ManuallyDrop,
	ops::{
		BitAnd,
		BitAndAssign,
		BitOr,
		BitOrAssign,
		BitXor,
		BitXorAssign,
		Deref,
		DerefMut,
		Index,
		IndexMut,
		Not,
	},
};

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitAnd<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitAndAssign<Rhs>,
{
	type Output = Self;

	#[inline]
	fn bitand(mut self, rhs: Rhs) -> Self::Output {
		*self.as_mut_bitslice() &= rhs;
		self
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitAndAssign<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitAndAssign<Rhs>,
{
	#[inline]
	fn bitand_assign(&mut self, rhs: Rhs) {
		*self.as_mut_bitslice() &= rhs;
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitOr<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitOrAssign<Rhs>,
{
	type Output = Self;

	#[inline]
	fn bitor(mut self, rhs: Rhs) -> Self::Output {
		*self.as_mut_bitslice() |= rhs;
		self
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitOrAssign<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitOrAssign<Rhs>,
{
	#[inline]
	fn bitor_assign(&mut self, rhs: Rhs) {
		*self.as_mut_bitslice() |= rhs;
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitXor<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitXorAssign<Rhs>,
{
	type Output = Self;

	#[inline]
	fn bitxor(mut self, rhs: Rhs) -> Self::Output {
		*self.as_mut_bitslice() ^= rhs;
		self
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Rhs> BitXorAssign<Rhs> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitXorAssign<Rhs>,
{
	#[inline]
	fn bitxor_assign(&mut self, rhs: Rhs) {
		*self.as_mut_bitslice() ^= rhs;
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> Deref for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	type Target = BitSlice<O, T>;

	#[inline(always)]
	fn deref(&self) -> &Self::Target {
		self.as_bitslice()
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> DerefMut for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline(always)]
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.as_mut_bitslice()
	}
}

impl<O, T> Drop for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn drop(&mut self) {
		//  Run the `Box` destructor to deällocate the buffer.
		self.with_box(|boxed| unsafe { ManuallyDrop::drop(boxed) });
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Idx> Index<Idx> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: Index<Idx>,
{
	type Output = <BitSlice<O, T> as Index<Idx>>::Output;

	#[inline]
	fn index(&self, index: Idx) -> &Self::Output {
		self.as_bitslice().index(index)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Idx> IndexMut<Idx> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: IndexMut<Idx>,
{
	#[inline]
	fn index_mut(&mut self, index: Idx) -> &mut Self::Output {
		self.as_mut_bitslice().index_mut(index)
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> Not for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	type Output = Self;

	#[inline]
	fn not(mut self) -> Self::Output {
		for elem in self.as_mut_slice().iter_mut().map(dvl::mem_mut) {
			*elem = !*elem;
		}
		self
	}
}
