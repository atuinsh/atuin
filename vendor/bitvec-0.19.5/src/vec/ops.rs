//! Operator implementations for `BitVec`.

use crate::{
	devel as dvl,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
	vec::BitVec,
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
impl<O, T, Rhs> BitAnd<Rhs> for BitVec<O, T>
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
impl<O, T, Rhs> BitAndAssign<Rhs> for BitVec<O, T>
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
impl<O, T, Rhs> BitOr<Rhs> for BitVec<O, T>
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
impl<O, T, Rhs> BitOrAssign<Rhs> for BitVec<O, T>
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
impl<O, T, Rhs> BitXor<Rhs> for BitVec<O, T>
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
impl<O, T, Rhs> BitXorAssign<Rhs> for BitVec<O, T>
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
impl<O, T> Deref for BitVec<O, T>
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
impl<O, T> DerefMut for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline(always)]
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.as_mut_bitslice()
	}
}

impl<O, T> Drop for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn drop(&mut self) {
		//  The buffer elements do not have destructors.
		self.clear();
		//  Run the `Vec` destructor to deällocate the buffer.
		self.with_vec(|vec| unsafe { ManuallyDrop::drop(vec) });
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T, Idx> Index<Idx> for BitVec<O, T>
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
impl<O, T, Idx> IndexMut<Idx> for BitVec<O, T>
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

/** This implementation inverts all elements in the live buffer. You cannot rely
on the value of bits in the buffer that are outside the domain of
`BitVec::as_mit_bitslice`.
**/
#[cfg(not(tarpaulin_include))]
impl<O, T> Not for BitVec<O, T>
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
