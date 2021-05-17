//! Operator trait implementations

use crate::{
	access::BitAccess,
	devel as dvl,
	domain::DomainMut,
	order::BitOrder,
	slice::{
		BitSlice,
		BitSliceIndex,
	},
	store::BitStore,
};

use core::ops::{
	BitAndAssign,
	BitOrAssign,
	BitXorAssign,
	Index,
	IndexMut,
	Not,
	Range,
	RangeFrom,
	RangeFull,
	RangeInclusive,
	RangeTo,
	RangeToInclusive,
};

use tap::pipe::Pipe;

impl<O, T, Rhs> BitAndAssign<Rhs> for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	Rhs: IntoIterator<Item = bool>,
{
	fn bitand_assign(&mut self, rhs: Rhs) {
		let mut iter = rhs.into_iter();
		self.for_each(|_, bit| bit & iter.next().unwrap_or(false));
	}
}

impl<O, T, Rhs> BitOrAssign<Rhs> for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	Rhs: IntoIterator<Item = bool>,
{
	fn bitor_assign(&mut self, rhs: Rhs) {
		let mut iter = rhs.into_iter();
		self.for_each(|_, bit| bit | iter.next().unwrap_or(false));
	}
}

impl<O, T, Rhs> BitXorAssign<Rhs> for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	Rhs: IntoIterator<Item = bool>,
{
	fn bitxor_assign(&mut self, rhs: Rhs) {
		let mut iter = rhs.into_iter();
		self.for_each(|_, bit| bit ^ iter.next().unwrap_or(false));
	}
}

impl<O, T> Index<usize> for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	type Output = bool;

	/// Looks up a single bit by semantic index.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![Msb0, u8; 0, 0, 0, 0, 0, 0, 0, 0, 1, 0];
	/// assert!(!bits[7]); // --------------------------^  |  |
	/// assert!( bits[8]); // -----------------------------^  |
	/// assert!(!bits[9]); // --------------------------------^
	/// ```
	///
	/// If the index is greater than or equal to the length, indexing will
	/// panic.
	///
	/// The below test will panic when accessing index 1, as only index 0 is
	/// valid.
	///
	/// ```rust,should_panic
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0,  ];
	/// bits[1]; // --------^
	/// ```
	fn index(&self, index: usize) -> &Self::Output {
		index.index(self)
	}
}

/// Generate `Index`/`Mut` implementations for subslicing.
macro_rules! index {
	($($t:ty),+ $(,)?) => { $(
		impl<O, T> Index<$t> for BitSlice<O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
			type Output = Self;

			fn index(&self, index: $t) -> &Self::Output {
				index.index(self)
			}
		}

		impl<O, T> IndexMut<$t> for BitSlice<O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
			fn index_mut(&mut self, index: $t) -> &mut Self::Output {
				index.index_mut(self)
			}
		}
	)+ };
}

//  Implement `Index`/`Mut` subslicing with all the ranges.
index!(
	Range<usize>,
	RangeFrom<usize>,
	RangeFull,
	RangeInclusive<usize>,
	RangeTo<usize>,
	RangeToInclusive<usize>,
);

impl<'a, O, T> Not for &'a mut BitSlice<O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	type Output = Self;

	fn not(self) -> Self::Output {
		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => elem
				.pipe(dvl::accessor)
				.invert_bits(dvl::alias_mask::<T>(O::mask(head, tail))),
			DomainMut::Region { head, body, tail } => {
				if let Some((head, elem)) = head {
					elem.pipe(dvl::accessor)
						.invert_bits(dvl::alias_mask::<T>(O::mask(head, None)));
				}
				for elem in body {
					*elem = !*elem;
				}
				if let Some((elem, tail)) = tail {
					elem.pipe(dvl::accessor)
						.invert_bits(dvl::alias_mask::<T>(O::mask(None, tail)));
				}
			},
		}
		self
	}
}
