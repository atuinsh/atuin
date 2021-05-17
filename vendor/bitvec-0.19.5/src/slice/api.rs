//! Port of the `[T]` function API.

use crate::{
	access::BitAccess,
	array::BitArray,
	devel as dvl,
	domain::{
		Domain,
		DomainMut,
	},
	index::BitRegister,
	mem::BitMemory,
	order::BitOrder,
	pointer::BitPtr,
	slice::{
		iter::{
			Chunks,
			ChunksExact,
			ChunksExactMut,
			ChunksMut,
			Iter,
			IterMut,
			RChunks,
			RChunksExact,
			RChunksExactMut,
			RChunksMut,
			RSplit,
			RSplitMut,
			RSplitN,
			RSplitNMut,
			Split,
			SplitMut,
			SplitN,
			SplitNMut,
			Windows,
		},
		BitMut,
		BitSlice,
	},
	store::BitStore,
};

use core::{
	cmp,
	ops::{
		Range,
		RangeBounds,
		RangeFrom,
		RangeFull,
		RangeInclusive,
		RangeTo,
		RangeToInclusive,
	},
};

use tap::{
	pipe::Pipe,
	tap::Tap,
};

#[cfg(feature = "alloc")]
use crate::vec::BitVec;

/// Port of the `[T]` inherent API.
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Returns the number of bits in the slice.
	///
	/// # Original
	///
	/// [`slice::len`](https://doc.rust-lang.org/std/primitive.slice.html#method.len)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// assert_eq!(bits![0].len(), 1);
	/// ```
	#[inline]
	pub fn len(&self) -> usize {
		self.bitptr().len()
	}

	/// Returns `true` if the slice has a length of 0.
	///
	/// # Original
	///
	/// [`slice::is_empty`](https://doc.rust-lang.org/std/primitive.slice.html#method.is_empty)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// assert!(bits![].is_empty());
	/// assert!(!bits![0].is_empty());
	/// ```
	#[inline]
	pub fn is_empty(&self) -> bool {
		/* TODO(myrrlyn): Investigate coercing all empty slices to `empty()`

		The empty slice pointer represents its entire `.len` field as zero,
		which removes a shift operation in the pointer decoding. `BitSlice` only
		monotonically decreases, so when it becomes empty, writing `0` to `.len`
		may be more advantageous than preserving the `head` component.
		*/
		self.bitptr().len() == 0
	}

	/// Returns the first bit of the slice, or `None` if it is empty.
	///
	/// # Original
	///
	/// [`slice::first`](https://doc.rust-lang.org/std/primitive.slice.html#method.first)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// assert_eq!(Some(&true), bits![1, 0].first());
	/// assert!(bits![].first().is_none());
	/// ```
	#[inline]
	pub fn first(&self) -> Option<&bool> {
		self.get(0)
	}

	/// Returns a mutable pointer to the first bit of the slice, or `None` if it
	/// is empty.
	///
	/// # Original
	///
	/// [`slice::first_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.first_mut)
	///
	/// # API Differences
	///
	/// This crate cannot manifest `&mut bool` references, and must use the
	/// `BitMut` proxy type where `&mut bool` exists in the standard library
	/// API. The proxy value must be bound as `mut` in order to write through
	/// it.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0];
	/// assert!(!bits[0]);
	/// if let Some(mut first) = bits.first_mut() {
	///   *first = true;
	/// }
	/// assert!(bits[0]);
	/// ```
	#[inline]
	pub fn first_mut(&mut self) -> Option<BitMut<O, T>> {
		self.get_mut(0)
	}

	/// Returns the first and all the rest of the bits of the slice, or `None`
	/// if it is empty.
	///
	/// # Original
	///
	/// [`slice::split_first`](https://doc.rust-lang.org/std/primitive.slice.html#split_first)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// if let Some((first, rest)) = bits![1].split_first() {
	///   assert!(*first);
	///   assert!(rest.is_empty());
	/// }
	/// ```
	#[inline]
	pub fn split_first(&self) -> Option<(&bool, &Self)> {
		match self.len() {
			0 => None,
			_ => unsafe {
				let (head, rest) = self.split_at_unchecked(1);
				Some((head.get_unchecked(0), rest))
			},
		}
	}

	/// Returns the first and all the rest of the bits of the slice, or `None`
	/// if it is empty.
	///
	/// # Original
	///
	/// [`slice::split_first_mut`](https://doc.rust-lang.org/std/primitive.slice.html#split_first_mut)
	///
	/// # API Differences
	///
	/// This crate cannot manifest `&mut bool` references, and must use the
	/// `BitMut` proxy type where `&mut bool` exists in the standard library
	/// API. The proxy value must be bound as `mut` in order to write through
	/// it.
	///
	/// Because the references are permitted to use the same memory address,
	/// they are marked as aliasing in order to satisfy Rust’s requirements
	/// about freedom from data races.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0; 3];
	/// if let Some((mut first, rest)) = bits.split_first_mut() {
	///   *first = true;
	///   *rest.get_mut(1).unwrap() = true;
	/// }
	/// assert_eq!(bits.count_ones(), 2);
	/// assert!(bits![mut].split_first_mut().is_none());
	/// ```
	#[inline]
	//  `pub type Aliased = BitSlice<O, T::Alias>;` is not allowed in inherents,
	//  so this will not be aliased.
	#[allow(clippy::type_complexity)]
	pub fn split_first_mut(
		&mut self,
	) -> Option<(BitMut<O, T::Alias>, &mut BitSlice<O, T::Alias>)> {
		match self.len() {
			0 => None,
			_ => unsafe {
				let (head, rest) = self.split_at_unchecked_mut(1);
				Some((head.get_unchecked_mut(0), rest))
			},
		}
	}

	/// Returns the last and all the rest of the bits of the slice, or `None` if
	/// it is empty.
	///
	/// # Original
	///
	/// [`slice::split_last`](https://doc.rust-lang.org/std/primitive.slice.html#method.split_last)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1];
	/// if let Some((last, rest)) = bits.split_last() {
	///   assert!(*last);
	///   assert!(rest.is_empty());
	/// }
	/// ```
	#[inline]
	pub fn split_last(&self) -> Option<(&bool, &Self)> {
		match self.len() {
			0 => None,
			len => unsafe {
				let (rest, tail) = self.split_at_unchecked(len.wrapping_sub(1));
				Some((tail.get_unchecked(0), rest))
			},
		}
	}

	/// Returns the last and all the rest of the bits of the slice, or `None` if
	/// it is empty.
	///
	/// # Original
	///
	/// [`slice::split_last_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.split_last_mut)
	///
	/// # API Differences
	///
	/// This crate cannot manifest `&mut bool` references, and must use the
	/// `BitMut` proxy type where `&mut bool` exists in the standard library
	/// API. The proxy value must be bound as `mut` in order to write through
	/// it.
	///
	/// Because the references are permitted to use the same memory address,
	/// they are marked as aliasing in order to satisfy Rust’s requirements
	/// about freedom from data races.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0; 3];
	///
	/// if let Some((mut last, rest)) = bits.split_last_mut() {
	///   *last = true;
	///   *rest.get_mut(1).unwrap() = true;
	/// }
	/// assert_eq!(bits.count_ones(), 2);
	/// assert!(bits![mut].split_last_mut().is_none());
	/// ```
	#[inline]
	//  `pub type Aliased = BitSlice<O, T::Alias>;` is not allowed in inherents,
	//  so this will not be aliased.
	#[allow(clippy::type_complexity)]
	pub fn split_last_mut(
		&mut self,
	) -> Option<(BitMut<O, T::Alias>, &mut BitSlice<O, T::Alias>)> {
		match self.len() {
			0 => None,
			len => unsafe {
				let (rest, tail) = self.split_at_unchecked_mut(len - 1);
				Some((tail.get_unchecked_mut(0), rest))
			},
		}
	}

	/// Returns the last bit of the slice, or `None` if it is empty.
	///
	/// # Original
	///
	/// [`slice::last`](https://doc.rust-lang.org/std/primitive.slice.html#method.last)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// assert_eq!(Some(&true), bits![0, 1].last());
	/// assert!(bits![].last().is_none());
	/// ```
	#[inline]
	pub fn last(&self) -> Option<&bool> {
		match self.len() {
			0 => None,
			len => Some(unsafe { self.get_unchecked(len - 1) }),
		}
	}

	/// Returns a mutable pointer to the last bit of the slice, or `None` if it
	/// is empty.
	///
	/// # Original
	///
	/// [`slice::last_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.last_mut)
	///
	/// # API Differences
	///
	/// This crate cannot manifest `&mut bool` references, and must use the
	/// `BitMut` proxy type where `&mut bool` exists in the standard library
	/// API. The proxy value must be bound as `mut` in order to write through
	/// it.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0];
	/// if let Some(mut last) = bits.last_mut() {
	///   *last = true;
	/// }
	/// assert!(bits[0]);
	/// ```
	#[inline]
	pub fn last_mut(&mut self) -> Option<BitMut<O, T>> {
		match self.len() {
			0 => None,
			len => Some(unsafe { self.get_unchecked_mut(len - 1) }),
		}
	}

	/// Returns a reference to an element or subslice depending on the type of
	/// index.
	///
	/// - If given a position, returns a reference to the element at that
	///   position or `None` if out of bounds.
	/// - If given a range, returns the subslice corresponding to that range, or
	///   `None` if out of bounds.
	///
	/// # Original
	///
	/// [`slice::get`](https://doc.rust-lang.org/std/primitive.slice.html#method.get)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 0];
	///
	/// assert_eq!(Some(&true), bits.get(1));
	/// assert_eq!(Some(&bits[1 .. 3]), bits.get(1 .. 3));
	/// assert!(bits.get(9).is_none());
	/// assert!(bits.get(8 .. 10).is_none());
	/// ```
	#[inline]
	pub fn get<'a, I>(&'a self, index: I) -> Option<I::Immut>
	where I: BitSliceIndex<'a, O, T> {
		index.get(self)
	}

	/// Returns a mutable reference to an element or subslice depending on the
	/// type of index (see [`get`]) or `None` if the index is out of bounds.
	///
	/// # Original
	///
	/// [`slice::get_mut`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.get_mut)
	///
	/// # API Differences
	///
	/// When `I` is `usize`, this returns `BitMut` instead of `&mut bool`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0; 2];
	/// assert!(!bits.get(1).unwrap());
	/// *bits.get_mut(1).unwrap() = true;
	/// assert!(bits.get(1).unwrap());
	/// ```
	///
	/// [`get`]: #method.get
	#[inline]
	pub fn get_mut<'a, I>(&'a mut self, index: I) -> Option<I::Mut>
	where I: BitSliceIndex<'a, O, T> {
		index.get_mut(self)
	}

	/// Returns a reference to an element or subslice, without doing bounds
	/// checking.
	///
	/// This is generally not recommended; use with caution!
	///
	/// Unlike the original slice function, calling this with an out-of-bounds
	/// index is not *technically* compile-time [undefined behavior], as the
	/// references produced do not actually describe local memory. However, the
	/// use of an out-of-bounds index will eventually cause an out-of-bounds
	/// memory read, which is a runtime safety violation. For a safe alternative
	/// see [`get`].
	///
	/// # Original
	///
	/// [`slice::get_unchecked`](https://doc.rust-lang.org/std/primitive.slice.html#method.get_unchecked)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1];
	/// unsafe {
	///   assert!(*bits.get_unchecked(1));
	/// }
	/// ```
	///
	/// [`get`]: #method.get
	/// [undefined behavior]: https://doc.rust-lang.org/reference/behavior-considered-undefined.html
	#[inline]
	#[allow(clippy::missing_safety_doc)]
	pub unsafe fn get_unchecked<'a, I>(&'a self, index: I) -> I::Immut
	where I: BitSliceIndex<'a, O, T> {
		index.get_unchecked(self)
	}

	/// Returns a mutable reference to the output at this location, without
	/// doing bounds checking.
	///
	/// This is generally not recommended; use with caution!
	///
	/// Unlike the original slice function, calling this with an out-of-bounds
	/// index is not *technically* compile-time [undefined behavior], as the
	/// references produced do not actually describe local memory. However, the
	/// use of an out-of-bounds index will eventually cause an out-of-bounds
	/// memory write, which is a runtime safety violation. For a safe
	/// alternative see [`get_mut`].
	///
	/// # Original
	///
	/// [`slice::get_unchecked_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.get_unchecked_mut)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0; 2];
	/// unsafe {
	///   let mut bit = bits.get_unchecked_mut(1);
	///   *bit = true;
	/// }
	/// assert!(bits[1]);
	/// ```
	///
	/// [`get_mut`]: #method.get_mut
	/// [undefined behavior]: ../../reference/behavior-considered-undefined.html
	#[inline]
	#[allow(clippy::missing_safety_doc)]
	pub unsafe fn get_unchecked_mut<'a, I>(&'a mut self, index: I) -> I::Mut
	where I: BitSliceIndex<'a, O, T> {
		index.get_unchecked_mut(self)
	}

	/// Returns a raw bit-slice pointer to the region.
	///
	/// The caller must ensure that the slice outlives the pointer this function
	/// returns, or else it will end up pointing to garbage.
	///
	/// The caller must also ensure that the memory the pointer
	/// (non-transitively) points to is only written to if `T` allows shared
	/// mutation, using this pointer or any pointer derived from it. If you need
	/// to mutate the contents of the slice, use [`as_mut_ptr`].
	///
	/// Modifying the container (such as `BitVec`) referenced by this slice may
	/// cause its buffer to be reällocated, which would also make any pointers
	/// to it invalid.
	///
	/// # Original
	///
	/// [`slice::as_ptr`](https://doc.rust-lang.org/std/primitive.slice.html#method.as_ptr)
	///
	/// # API Differences
	///
	/// This returns `*const BitSlice`, which is the equivalent of `*const [T]`
	/// instead of `*const T`. The pointer encoding used requires more than one
	/// CPU word of space to address a single bit, so there is no advantage to
	/// removing the length information from the encoded pointer value.
	///
	/// # Notes
	///
	/// You **cannot** use any of the methods in the `pointer` fundamental type
	/// or the `core::ptr` module on the `*_ BitSlice` type. This pointer
	/// retains the `bitvec`-specific value encoding, and is incomprehensible by
	/// the Rust standard library.
	///
	/// The only thing you can do with this pointer is dereference it.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 1, 0];
	/// let bits_ptr = bits.as_ptr();
	///
	/// for i in 0 .. bits.len() {
	///   assert_eq!(bits[i], unsafe {
	///     (&*bits_ptr)[i]
	///   });
	/// }
	/// ```
	///
	/// [`as_mut_ptr`]: #method.as_mut_ptr
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_ptr(&self) -> *const Self {
		self as *const Self
	}

	/// Returns an unsafe mutable bit-slice pointer to the region.
	///
	/// The caller must ensure that the slice outlives the pointer this function
	/// returns, or else it will end up pointing to garbage.
	///
	/// Modifying the container (such as `BitVec`) referenced by this slice may
	/// cause its buffer to be reällocated, which would also make any pointers
	/// to it invalid.
	///
	/// # Original
	///
	/// [`slice::as_mut_ptr`](https://doc.rust-lang.org/std/primitive.slice.html#method.as_mut_ptr)
	///
	/// # API Differences
	///
	/// This returns `*mut BitSlice`, which is the equivalont of `*mut [T]`
	/// instead of `*mut T`. The pointer encoding used requires more than one
	/// CPU word of space to address a single bit, so there is no advantage to
	/// removing the length information from the encoded pointer value.
	///
	/// # Notes
	///
	/// You **cannot** use any of the methods in the `pointer` fundamental type
	/// or the `core::ptr` module on the `*_ BitSlice` type. This pointer
	/// retains the `bitvec`-specific value encoding, and is incomprehensible by
	/// the Rust standard library.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Lsb0, u8; 0; 8];
	/// let bits_ptr = bits.as_mut_ptr();
	///
	/// for i in 0 .. bits.len() {
	///   unsafe { &mut *bits_ptr }.set(i, i % 3 == 0);
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b0100_1001);
	/// ```
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_ptr(&mut self) -> *mut Self {
		self as *mut Self
	}

	/// Swaps two bits in the slice.
	///
	/// # Original
	///
	/// [`slice::swap`](https://doc.rust-lang.org/std/primitive.slice.html#method.swap)
	///
	/// # Arguments
	///
	/// - `a`: The index of the first bit
	/// - `b`: The index of the second bit
	///
	/// # Panics
	///
	/// Panics if `a` or `b` are out of bounds.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0, 1];
	/// bits.swap(0, 1);
	/// assert_eq!(bits, bits![1, 0]);
	/// ```
	#[inline]
	pub fn swap(&mut self, a: usize, b: usize) {
		let len = self.len();
		assert!(a < len, "Index {} out of bounds: {}", a, len);
		assert!(b < len, "Index {} out of bounds: {}", b, len);
		unsafe {
			self.swap_unchecked(a, b);
		}
	}

	/// Reverses the order of bits in the slice, in place.
	///
	/// # Original
	///
	/// [`slice::reverse`](https://doc.rust-lang.org/std/primitive.slice.html#method.reverse)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0b1_1001100u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits[1 ..].reverse();
	/// assert_eq!(data, 0b1_0011001);
	/// ```
	#[inline]
	pub fn reverse(&mut self) {
		/* This would be better written as a recursive algorithm that swaps the
		edge bits and recurses on `[1 .. len - 1]`, but Rust does not guarantee
		tail-call optimization, and manual iteration allows for slight
		performance optimization on the range reduction.

		Begin with raw pointer manipulation. That’s how you know this is a good
		function.
		*/
		let mut bitptr = self.bitptr();
		//  The length does not need to be encoded into, and decoded back out
		//  of, the pointer at each iteration. It is just a loop counter.
		let mut len = bitptr.len();
		//  Reversing 1 or 0 bits has no effect.
		while len > 1 {
			unsafe {
				//  Bring `len` from one past the last to the last exactly.
				len -= 1;
				//  Swap the 0 and last indices.
				bitptr.to_bitslice_mut::<O>().swap_unchecked(0, len);

				//  Move the pointer upwards by one bit.
				bitptr.incr_head();
				//  `incr_head` slides the tail up by one, so decrease it again.
				len -= 1;

				//  TODO(myrrlyn): See if range subslicing can be made faster
				//  than this unpacking.
			}
		}
	}

	/// Returns an iterator over the slice.
	///
	/// # Original
	///
	/// [`slice::iter`](https://doc.rust-lang.org/std/primitive.slice.html#method.iter)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 0, 0, 0, 0, 1];
	/// let mut iterator = bits.iter();
	///
	/// assert_eq!(iterator.next(), Some(&false));
	/// assert_eq!(iterator.next(), Some(&true));
	/// assert_eq!(iterator.nth(5), Some(&true));
	/// assert_eq!(iterator.next(), None);
	/// ```
	#[inline]
	pub fn iter(&self) -> Iter<O, T> {
		self.into_iter()
	}

	/// Returns an iterator that allows modifying each bit.
	///
	/// # Original
	///
	/// [`slice::iter_mut`](https://doc.rust-lang.org/std/primitive.slice.html#Method.iter_mut)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0; 8];
	/// for (idx, mut elem) in bits.iter_mut().enumerate() {
	///   *elem = idx % 3 == 0;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b100_100_10);
	/// ```
	#[inline]
	pub fn iter_mut(&mut self) -> IterMut<O, T> {
		self.into_iter()
	}

	/// Returns an iterator over all contiguous windows of length `size`. The
	/// windows overlap. If the slice is shorter than `size`, the iterator
	/// returns no values.
	///
	/// # Original
	///
	/// [`slice::windows`](https://doc.rust-lang.org/std/primitive.slice.html#method.windows)
	///
	/// # Panics
	///
	/// Panics if `size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 0, 1, 0, 0, 1, 0, 1];
	/// let mut iter = bits.windows(6);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[.. 6]);
	/// assert_eq!(iter.next().unwrap(), &bits[1 .. 7]);
	/// assert_eq!(iter.next().unwrap(), &bits[2 ..]);
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// If the slice is shorter than `size`:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = BitSlice::<LocalBits, usize>::empty();
	/// let mut iter = bits.windows(1);
	/// assert!(iter.next().is_none());
	/// ```
	#[inline]
	pub fn windows(&self, size: usize) -> Windows<O, T> {
		assert_ne!(size, 0, "Window width cannot be 0");
		Windows::new(self, size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the beginning of the slice.
	///
	/// The chunks are slices and do not overlap. If `chunk_size` does not
	/// divide the length of the slice, then the last chunk will not have length
	/// `chunk_size`.
	///
	/// See [`chunks_exact`] for a variant of this iterator that returns chunks
	/// of always exactly `chunk_size` bits, and [`rchunks`] for the same
	/// iterator but starting at the end of the slice.
	///
	/// # Original
	///
	/// [`slice::chunks`](https://doc.rust-lang.org/std/primitive.slice.html#method.chunks)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 0, 1, 1, 1, 1, 0, 0];
	/// let mut iter = bits.chunks(3);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[.. 3]);
	/// assert_eq!(iter.next().unwrap(), &bits[3 .. 6]);
	/// assert_eq!(iter.next().unwrap(), &bits[6 ..]);
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// [`chunks_exact`]: #method.chunks_exact
	/// [`rchunks`]: #method.rchunks
	#[inline]
	pub fn chunks(&self, chunk_size: usize) -> Chunks<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		Chunks::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the beginning of the slice.
	///
	/// The chunks are mutable slices, and do not overlap. If `chunk_size` does
	/// not divide the length of the slice, then the last chunk will not have
	/// length `chunk_size`.
	///
	/// See [`chunks_exact_mut`] for a variant of this iterator that returns
	/// chunks of always exactly `chunk_size` bits, and [`rchunks_mut`] for the
	/// same iterator but starting at the end of the slice.
	///
	/// # Original
	///
	/// [`slice::chunks_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.chunks_mut)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Lsb0, u8; 0; 8];
	/// for (idx, chunk) in bits.chunks_mut(3).enumerate() {
	///   chunk.set(2 - idx, true);
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b01_010_100);
	/// ```
	///
	/// [`chunks_exact_mut`]: #method.chunks_exact_mut
	/// [`rchunks_mut`]: #method.rchunks_mut
	#[inline]
	pub fn chunks_mut(&mut self, chunk_size: usize) -> ChunksMut<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		ChunksMut::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the beginning of the slice.
	///
	/// The chunks are slices and do not overlap. If `chunk_size` does not
	/// divide the length of the slice, then the last up to `chunk_size-1` bits
	/// will be omitted and can be retrieved from the `remainder` function of
	/// the iterator.
	///
	/// Due to each chunk having exactly `chunk_size` bits, the compiler may
	/// optimize the resulting code better than in the case of [`chunks`].
	///
	/// See [`chunks`] for a variant of this iterator that also returns the
	/// remainder as a smaller chunk, and [`rchunks_exact`] for the same
	/// iterator but starting at the end of the slice.
	///
	/// # Original
	///
	/// [`slice::chunks_exact`](https://doc.rust-lang.org/std/primitive.slice.html#method.chunks_exact)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 0, 1, 0, 1, 1];
	/// let mut iter = bits.chunks_exact(3);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[.. 3]);
	/// assert_eq!(iter.next().unwrap(), &bits[3 .. 6]);
	/// assert!(iter.next().is_none());
	/// assert_eq!(iter.remainder(), &bits[6 ..]);
	/// ```
	///
	/// [`chunks`]: #method.chunks
	/// [`rchunks_exact`]: #method.rchunks_exact
	#[inline]
	pub fn chunks_exact(&self, chunk_size: usize) -> ChunksExact<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		ChunksExact::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the beginning of the slice.
	///
	/// The chunks are mutable slices, and do not overlap. If `chunk_size` does
	/// not divide the beginning length of the slice, then the last up to
	/// `chunk_size-1` bits will be omitted and can be retrieved from the
	/// `into_remainder` function of the iterator.
	///
	/// Due to each chunk having exactly `chunk_size` bits, the compiler may
	/// optimize the resulting code better than in the case of [`chunks_mut`].
	///
	/// See [`chunks_mut`] for a variant of this iterator that also returns the
	/// remainder as a smaller chunk, and [`rchunks_exact_mut`] for the same
	/// iterator but starting at the end of the slice.
	///
	/// # Original
	///
	/// [`slice::chunks_exact_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.chunks_exact_mut)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Lsb0, u8; 0; 8];
	/// for (idx, chunk) in bits.chunks_exact_mut(3).enumerate() {
	///   chunk.set(idx, true);
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b00_010_001);
	/// ```
	///
	/// [`chunks_mut`]: #method.chunks_mut
	/// [`rchunks_exact_mut`]: #method.rchunks_exact_mut
	#[inline]
	pub fn chunks_exact_mut(
		&mut self,
		chunk_size: usize,
	) -> ChunksExactMut<O, T>
	{
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		ChunksExactMut::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the end of the slice.
	///
	/// The chunks are slices and do not overlap. If `chunk_size` does not
	/// divide the length of the slice, then the last chunk will not have length
	/// `chunk_size`.
	///
	/// See [`rchunks_exact`] for a variant of this iterator that returns chunks
	/// of always exactly `chunk_size` bits, and [`chunks`] for the same
	/// iterator but starting at the beginning of the slice.
	///
	/// # Original
	///
	/// [`slice::rchunks`](https://doc.rust-lang.org/std/primitive.slice.html#method.rchunks)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 0, 1, 1, 0, 1, 1, 0];
	/// let mut iter = bits.rchunks(3);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[5 ..]);
	/// assert_eq!(iter.next().unwrap(), &bits[2 .. 5]);
	/// assert_eq!(iter.next().unwrap(), &bits[.. 2]);
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// [`chunks`]: #method.chunks
	/// [`rchunks_exact`]: #method.rchunks_exact
	#[inline]
	pub fn rchunks(&self, chunk_size: usize) -> RChunks<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		RChunks::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the end of the slice.
	///
	/// The chunks are mutable slices, and do not overlap. If `chunk_size` does
	/// not divide the length of the slice, then the last chunk will not have
	/// length `chunk_size`.
	///
	/// See [`rchunks_exact_mut`] for a variant of this iterator that returns
	/// chunks of always exactly `chunk_size` bits, and [`chunks_mut`] for the
	/// same iterator but starting at the beginning of the slice.
	///
	/// # Original
	///
	/// [`slice::rchunks_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.rchunks_mut)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Lsb0, u8; 0; 8];
	/// for (idx, chunk) in bits.rchunks_mut(3).enumerate() {
	///   chunk.set(2 - idx, true);
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b100_010_01);
	/// ```
	///
	/// [`chunks_mut`]: #method.chunks_mut
	/// [`rchunks_exact_mut`]: #method.rchunks_exact_mut
	#[inline]
	pub fn rchunks_mut(&mut self, chunk_size: usize) -> RChunksMut<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		RChunksMut::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the end of the slice.
	///
	/// The chunks are slices and do not overlap. If `chunk_size` does not
	/// divide the length of the slice, then the last up to `chunk_size-1` bits
	/// will be omitted and can be retrieved from the `remainder` function of
	/// the iterator.
	///
	/// Due to each chunk having exactly `chunk_size` bits, the compiler can
	/// often optimize the resulting code better than in the case of [`chunks`].
	///
	/// See [`rchunks`] for a variant of this iterator that also returns the
	/// remainder as a smaller chunk, and [`chunks_exact`] for the same iterator
	/// but starting at the beginning of the slice.
	///
	/// # Original
	///
	/// [`slice::rchunks_exact`](https://doc.rust-lang.org/std/primitive.slice.html#method.rchunks_exact)
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0, 1, 1, 1, 0, 0, 1, 0];
	/// let mut iter = bits.rchunks_exact(3);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[5 ..]);
	/// assert_eq!(iter.next().unwrap(), &bits[2 .. 5]);
	/// assert!(iter.next().is_none());
	/// assert_eq!(iter.remainder(), &bits[.. 2]);
	/// ```
	///
	/// [`chunks`]: #method.chunks
	/// [`rchunks`]: #method.rchunks
	/// [`chunks_exact`]: #method.chunks_exact
	#[inline]
	pub fn rchunks_exact(&self, chunk_size: usize) -> RChunksExact<O, T> {
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		RChunksExact::new(self, chunk_size)
	}

	/// Returns an iterator over `chunk_size` bits of the slice at a time,
	/// starting at the end of the slice.
	///
	/// The chunks are mutable slices, and do not overlap. If `chunk_size` does
	/// not divide the length of the slice, then the last up to `chunk_size-1`
	/// bits will be omitted and can be retrieved from the `into_remainder`
	/// function of the iterator.
	///
	/// Due to each chunk having exactly `chunk_size` bits, the compiler can
	/// often optimize the resulting code better than in the case of
	/// [`chunks_mut`].
	///
	/// See [`rchunks_mut`] for a variant of this iterator that also returns the
	/// remainder as a smaller chunk, and [`chunks_exact_mut`] for the same
	/// iterator but starting at the beginning of the slice.
	///
	/// # Panics
	///
	/// Panics if `chunk_size` is 0.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Lsb0, u8; 0; 8];
	/// for (idx, chunk) in bits.rchunks_exact_mut(3).enumerate() {
	///   chunk.set(idx, true);
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b001_010_00);
	/// ```
	///
	/// [`chunks_mut`]: #method.chunks_mut
	/// [`rchunks_mut`]: #method.rchunks_mut
	/// [`chunks_exact_mut`]: #method.chunks_exact_mut
	#[inline]
	pub fn rchunks_exact_mut(
		&mut self,
		chunk_size: usize,
	) -> RChunksExactMut<O, T>
	{
		assert_ne!(chunk_size, 0, "Chunk width cannot be 0");
		RChunksExactMut::new(self, chunk_size)
	}

	/// Divides one slice into two at an index.
	///
	/// The first will contain all indices from `[0, mid)` (excluding the index
	/// `mid` itself) and the second will contain all indices from `[mid, len)`
	/// (excluding the index `len` itself).
	///
	/// # Original
	///
	/// [`slice::split_at`](https://doc.rust-lang.org/std/primitive.slice.html#method.split_at)
	///
	/// # Panics
	///
	/// Panics if `mid > len`.
	///
	/// # Behavior
	///
	/// When `mid` is `0` or `self.len()`, then the left or right return values,
	/// respectively, are empty slices. Empty slice references produced by this
	/// method are specified to have the address information you would expect:
	/// a left empty slice has the same base address and start bit as `self`,
	/// and a right empty slice will have its address raised by `self.len()`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 0, 0, 0, 1, 1];
	///
	/// let (left, right) = bits.split_at(0);
	/// assert!(left.is_empty());
	/// assert_eq!(right, bits);
	///
	/// let (left, right) = bits.split_at(2);
	/// assert_eq!(left, &bits[.. 2]);
	/// assert_eq!(right, &bits[2 ..]);
	///
	/// let (left, right) = bits.split_at(8);
	/// assert_eq!(left, bits);
	/// assert!(right.is_empty());
	/// ```
	#[inline]
	pub fn split_at(&self, mid: usize) -> (&Self, &Self) {
		let len = self.len();
		assert!(mid <= len, "Index {} out of bounds: {}", mid, len);
		unsafe { self.split_at_unchecked(mid) }
	}

	/// Divides one mutable slice into two at an index.
	///
	/// The first will contain all indices from `[0, mid)` (excluding the index
	/// `mid` itself) and the second will contain all indices from `[mid, len)`
	/// (excluding the index `len` itself).
	///
	/// # Original
	///
	/// [`slice::split_at_mut`](https://doc.rust-lang.org/std/primitive.html#method.split_at_mut)
	///
	/// # API Differences
	///
	/// Because the partition point `mid` is permitted to occur in the interior
	/// of a memory element `T`, this method is required to mark the returned
	/// slices as being to aliased memory. This marking ensures that writes to
	/// the covered memory use the appropriate synchronization behavior of your
	/// build to avoid data races – by default, this makes all writes atomic; on
	/// builds with the `atomic` feature disabled, this uses `Cell`s and
	/// forbids the produced subslices from leaving the current thread.
	///
	/// See the [`BitStore`] documentation for more information.
	///
	/// # Panics
	///
	/// Panics if `mid > len`.
	///
	/// # Behavior
	///
	/// When `mid` is `0` or `self.len()`, then the left or right return values,
	/// respectively, are empty slices. Empty slice references produced by this
	/// method are specified to have the address information you would expect:
	/// a left empty slice has the same base address and start bit as `self`,
	/// and a right empty slice will have its address raised by `self.len()`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0; 8];
	/// // scoped to restrict the lifetime of the borrows
	/// {
	///   let (left, right) = bits.split_at_mut(3);
	///   *left.get_mut(1).unwrap() = true;
	///   *right.get_mut(2).unwrap() = true;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b010_00100);
	/// ```
	///
	/// [`BitStore`]: ../store/trait.BitStore.html
	#[inline]
	//  `pub type Aliased = BitSlice<O, T::Alias>;` is not allowed in inherents,
	//  so this will not be aliased.
	#[allow(clippy::type_complexity)]
	pub fn split_at_mut(
		&mut self,
		mid: usize,
	) -> (&mut BitSlice<O, T::Alias>, &mut BitSlice<O, T::Alias>)
	{
		let len = self.len();
		assert!(mid <= len, "Index {} out of bounds: {}", mid, len);
		unsafe { self.split_at_unchecked_mut(mid) }
	}

	/// Returns an iterator over subslices separated by bits that match `pred`.
	/// The matched bit is not contained in the subslices.
	///
	/// # Original
	///
	/// [`slice::split`](https://doc.rust-lang.org/std/primitive.slice.html#method.split)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 0, 1, 0, 0, 0];
	/// let mut iter = bits.split(|_pos, bit| *bit);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[.. 1]);
	/// assert_eq!(iter.next().unwrap(), &bits[2 .. 4]);
	/// assert_eq!(iter.next().unwrap(), &bits[5 ..]);
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// If the first bit is matched, an empty slice will be the first item
	/// returned by the iterator. Similarly, if the last element in the slice is
	/// matched, an empty slice will be the last item returned by the iterator:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 0, 0, 1];
	/// let mut iter = bits.split(|_pos, bit| *bit);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[.. 3]);
	/// assert!(iter.next().unwrap().is_empty());
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// If two matched bits are directly adjacent, an empty slice will be
	/// present between them:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 0, 1, 1, 0, 0, 0, 0,];
	/// let mut iter = bits.split(|pos, bit| *bit);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[0 .. 2]);
	/// assert!(iter.next().unwrap().is_empty());
	/// assert_eq!(iter.next().unwrap(), &bits[4 .. 8]);
	/// assert!(iter.next().is_none());
	/// ```
	#[inline]
	pub fn split<F>(&self, pred: F) -> Split<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		Split::new(self, pred)
	}

	/// Returns an iterator over mutable subslices separated by bits that match
	/// `pred`. The matched bit is not contained in the subslices.
	///
	/// # Original
	///
	/// [`slice::split_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.split_mut)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0, 0, 1, 0, 0, 0, 1, 0];
	/// for group in bits.split_mut(|_pos, bit| *bit) {
	///   *group.get_mut(0).unwrap() = true;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b101_100_11);
	/// ```
	#[inline]
	pub fn split_mut<F>(&mut self, pred: F) -> SplitMut<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		SplitMut::new(self.alias_mut(), pred)
	}

	/// Returns an iterator over subslices separated by bits that match `pred`,
	/// starting at the end of the slice and working backwards. The matched bit
	/// is not contained in the subslices.
	///
	/// # Original
	///
	/// [`slice::rsplit`](https://doc.rust-lang.org/std/primitive.slice.html#method.rsplit)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0, 0, 0, 1, 0, 0, 0, 0];
	/// let mut iter = bits.rsplit(|_pos, bit| *bit);
	///
	/// assert_eq!(iter.next().unwrap(), &bits[4 ..]);
	/// assert_eq!(iter.next().unwrap(), &bits[.. 3]);
	/// assert!(iter.next().is_none());
	/// ```
	///
	/// As with `split()`, if the first or last bit is matched, an empty slice
	/// will be the first (or last) item returned by the iterator.
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 1, 0, 0, 1, 0, 0, 0, 1];
	/// let mut iter = bits.rsplit(|_pos, bit| *bit);
	///
	/// assert!(iter.next().unwrap().is_empty());
	/// assert_eq!(iter.next().unwrap(), &bits[4 .. 7]);
	/// assert_eq!(iter.next().unwrap(), &bits[1 .. 3]);
	/// assert!(iter.next().unwrap().is_empty());
	/// assert!(iter.next().is_none());
	/// ```
	#[inline]
	pub fn rsplit<F>(&self, pred: F) -> RSplit<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		RSplit::new(self, pred)
	}

	/// Returns an iterator over mutable subslices separated by bits that match
	/// `pred`, starting at the end of the slice and working backwards. The
	/// matched bit is not contained in the subslices.
	///
	/// # Original
	///
	/// [`slice::rsplit_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.rsplit_mut)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0, 0, 1, 0, 0, 0, 1, 0];
	/// for group in bits.rsplit_mut(|_pos, bit| *bit) {
	///   *group.get_mut(0).unwrap() = true;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b101_100_11);
	/// ```
	#[inline]
	pub fn rsplit_mut<F>(&mut self, pred: F) -> RSplitMut<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		RSplitMut::new(self.alias_mut(), pred)
	}

	/// Returns an iterator over subslices separated by bits that match `pred`,
	/// limited to returning at most `n` items. The matched bit is not contained
	/// in the subslices.
	///
	/// The last item returned, if any, will contain the remainder of the slice.
	///
	/// # Original
	///
	/// [`slice::splitn`](https://doc.rust-lang.org/std/primitive.slice.html#method.splitn)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 0, 1, 0, 0, 1, 0, 1];
	/// for group in bits.splitn(2, |pos, _bit| pos % 3 == 2) {
	/// # #[cfg(feature = "std")] {
	///   println!("{}", group.len());
	/// # }
	/// }
	/// //  2
	/// //  5
	/// # //  [10]
	/// # //  [00101]
	/// ```
	#[inline]
	pub fn splitn<F>(&self, n: usize, pred: F) -> SplitN<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		SplitN::new(self, pred, n)
	}

	/// Returns an iterator over subslices separated by bits that match `pred`,
	/// limited to returning at most `n` items. The matched element is not
	/// contained in the subslices.
	///
	/// The last item returned, if any, will contain the remainder of the slice.
	///
	/// # Original
	///
	/// [`slice::splitn_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.splitn_mut)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0, 0, 1, 0, 0, 0, 1, 0];
	/// for group in bits.splitn_mut(2, |_pos, bit| *bit) {
	///   *group.get_mut(0).unwrap() = true;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b101_100_10);
	/// ```
	#[inline]
	pub fn splitn_mut<F>(&mut self, n: usize, pred: F) -> SplitNMut<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		SplitNMut::new(self.alias_mut(), pred, n)
	}

	/// Returns an iterator over subslices separated by bits that match `pred`
	/// limited to returining at most `n` items. This starts at the end of the
	/// slice and works backwards. The matched bit is not contained in the
	/// subslices.
	///
	/// The last item returned, if any, will contain the remainder of the slice.
	///
	/// # Original
	///
	/// [`slice::rsplitn`](https://doc.rust-lang.org/std/primitive.slice.html#method.rsplitn)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![Msb0, u8; 1, 0, 1, 0, 0, 1, 0, 1];
	/// for group in bits.rsplitn(2, |pos, _bit| pos % 3 == 2) {
	/// # #[cfg(feature = "std")] {
	///   println!("{}", group.len());
	/// # }
	/// }
	/// //  2
	/// //  5
	/// # //  [10]
	/// # //  [00101]
	/// ```
	#[inline]
	pub fn rsplitn<F>(&self, n: usize, pred: F) -> RSplitN<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		RSplitN::new(self, pred, n)
	}

	/// Returns an iterator over subslices separated by bits that match `pred`
	/// limited to returning at most `n` items. This starts at the end of the
	/// slice and works backwards. The matched bit is not contained in the
	/// subslices.
	///
	/// The last item returned, if any, will contain the remainder of the slice.
	///
	/// # Original
	///
	/// [`slice::rsplitn_mut`](https://doc.rust-lang.org/std/primitive.slice.html#method.rsplitn_mut)
	///
	/// # API Differences
	///
	/// In order to allow more than one bit of information for the split
	/// decision, the predicate receives the index of each bit, as well as its
	/// value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut Msb0, u8; 0, 0, 1, 0, 0, 0, 1, 0];
	/// for group in bits.rsplitn_mut(2, |_pos, bit| *bit) {
	///   *group.get_mut(0).unwrap() = true;
	/// }
	/// assert_eq!(bits.as_slice()[0], 0b101_000_11);
	/// ```
	#[inline]
	pub fn rsplitn_mut<F>(&mut self, n: usize, pred: F) -> RSplitNMut<O, T, F>
	where F: FnMut(usize, &bool) -> bool {
		RSplitNMut::new(self.alias_mut(), pred, n)
	}

	/// Returns `true` if the slice contains a subslice that matches the given
	/// span.
	///
	/// # Original
	///
	/// [`slice::contains`](https://doc.rust-lang.org/std/primitive.slice.html#method.contains)
	///
	/// # API Differences
	///
	/// This searches for a matching subslice (allowing different type
	/// parameters) rather than for a specific bit. Searching for a contained
	/// element with a given value is not as useful on a collection of `bool`.
	///
	/// Furthermore, `BitSlice` defines [`any`] and [`not_all`], which are
	/// optimized searchers for any `true` or `false` bit, respectively, in a
	/// sequence.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0b0101_1010u8;
	/// let bits_msb = data.view_bits::<Msb0>();
	/// let bits_lsb = data.view_bits::<Lsb0>();
	/// assert!(bits_msb.contains(&bits_lsb[1 .. 5]));
	/// ```
	///
	/// This example uses a palindrome pattern to demonstrate that the slice
	/// being searched for does not need to have the same type parameters as the
	/// slice being searched.
	///
	/// [`any`]: #method.any
	/// [`not_all`]: #method.not_all
	#[inline]
	pub fn contains<O2, T2>(&self, x: &BitSlice<O2, T2>) -> bool
	where
		O2: BitOrder,
		T2: BitStore,
	{
		let len = x.len();
		if len > self.len() {
			return false;
		};
		self.windows(len).any(|s| s == x)
	}

	/// Returns `true` if `needle` is a prefix of the slice.
	///
	/// # Original
	///
	/// [`slice::starts_with`](https://doc.rust-lang.org/std/primitive.slice.html#method.starts_with)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0b0100_1011u8;
	/// let haystack = data.view_bits::<Msb0>();
	/// let needle = &data.view_bits::<Lsb0>()[2 .. 5];
	///
	/// assert!(haystack.starts_with(&needle[.. 2]));
	/// assert!(haystack.starts_with(needle));
	/// assert!(!haystack.starts_with(&haystack[2 .. 4]));
	/// ```
	///
	/// Always returns `true` if `needle` is an empty slice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let empty = BitSlice::<LocalBits, usize>::empty();
	/// assert!(0u8.view_bits::<LocalBits>().starts_with(empty));
	/// assert!(empty.starts_with(empty));
	/// ```
	#[inline]
	pub fn starts_with<O2, T2>(&self, needle: &BitSlice<O2, T2>) -> bool
	where
		O2: BitOrder,
		T2: BitStore,
	{
		let len = needle.len();
		self.len() >= len && needle == unsafe { self.get_unchecked(.. len) }
	}

	/// Returns `true` if `needle` is a suffix of the slice.
	///
	/// # Original
	///
	/// [`slice::ends_with`](https://doc.rust-lang.org/std/primitive.slice.html#method.ends_with)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0b0100_1011u8;
	/// let haystack = data.view_bits::<Lsb0>();
	/// let needle = &data.view_bits::<Msb0>()[3 .. 6];
	///
	/// assert!(haystack.ends_with(&needle[1 ..]));
	/// assert!(haystack.ends_with(needle));
	/// assert!(!haystack.ends_with(&haystack[2 .. 4]));
	/// ```
	///
	/// Always returns `true` if `needle` is an empty slice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let empty = BitSlice::<LocalBits, usize>::empty();
	/// assert!(0u8.view_bits::<LocalBits>().ends_with(empty));
	/// assert!(empty.ends_with(empty));
	/// ```
	#[inline]
	pub fn ends_with<O2, T2>(&self, needle: &BitSlice<O2, T2>) -> bool
	where
		O2: BitOrder,
		T2: BitStore,
	{
		let nlen = needle.len();
		let len = self.len();
		len >= nlen && needle == unsafe { self.get_unchecked(len - nlen ..) }
	}

	/// Rotates the slice in-place such that the first `by` bits of the slice
	/// move to the end while the last `self.len() - by` bits move to the front.
	/// After calling `rotate_left`, the bit previously at index `by` will
	/// become the first bit in the slice.
	///
	/// # Original
	///
	/// [`slice::rotate_left`](https://doc.rust-lang.org/std/primitive.slice.html#rotate_left)
	///
	/// # Panics
	///
	/// This function will panic if `by` is greater than the length of the
	/// slice. Note that `by == self.len()` does *not* panic and is a no-op
	/// rotation.
	///
	/// # Complexity
	///
	/// Takes linear (in `self.len()`) time.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0xF0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits.rotate_left(2);
	/// assert_eq!(data, 0xC3);
	/// ```
	///
	/// Rotating a subslice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0xF0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits[1 .. 5].rotate_left(1);
	/// assert_eq!(data, 0b1_1101_000);
	/// ```
	#[inline]
	pub fn rotate_left(&mut self, mut by: usize) {
		let len = self.len();
		assert!(
			by <= len,
			"Slices cannot be rotated by more than their length"
		);
		if by == 0 || by == len {
			return;
		}
		/* The standard one-element-at-a-time algorithm is necessary for `[T]`
		rotation, because it must not allocate, but bit slices have an advantage
		in that placing a single processor word on the stack as a temporary has
		significant logical acceleration.

		Instead, we can move `min(usize::BITS, by)` bits from the front of the
		slice into the stack, then shunt the rest of the slice downwards, then
		insert the stack bits into the now-open back, repeating until complete.

		There is no reason to use a stack temporary smaller than a processor
		word, so this uses `usize` instead of `T` for performance benefits.
		*/
		let mut tmp = BitArray::<O, usize>::new(0);
		while by > 0 {
			let shamt = cmp::min(<usize as BitMemory>::BITS as usize, by);
			unsafe {
				let tmp_bits = tmp.get_unchecked_mut(.. shamt);
				tmp_bits.clone_from_bitslice(self.get_unchecked(.. shamt));
				self.copy_within_unchecked(shamt .., 0);
				self.get_unchecked_mut(len - shamt ..)
					.clone_from_bitslice(tmp_bits);
			}
			by -= shamt;
		}
	}

	/// Rotates the slice in-place such that the first `self.len() - by` bits of
	/// the slice move to the end while the last `by` bits move to the front.
	/// After calling `rotate_right`, the bit previously at index `self.len() -
	/// by` will become the first bit in the slice.
	///
	/// # Original
	///
	/// [`slice::rotate_right`](https://doc.rust-lang.org/std/primitive.slice.html#rotate_right)
	///
	/// # Panics
	///
	/// This function will panic if `by` is greater than the length of the
	/// slice. Note that `by == self.len()` does *not* panic and is a no-op
	/// rotation.
	///
	/// # Complexity
	///
	/// Takes linear (in `self.len()`) time.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0xF0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits.rotate_right(2);
	/// assert_eq!(data, 0x3C);
	/// ```
	///
	/// Rotate a subslice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0xF0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits[1 .. 5].rotate_right(1);
	/// assert_eq!(data, 0b1_0111_000);
	/// ```
	#[inline]
	pub fn rotate_right(&mut self, mut by: usize) {
		let len = self.len();
		assert!(
			by <= len,
			"Slices cannot be rotated by more than their length"
		);
		if by == 0 || by == len {
			return;
		}
		let mut tmp = BitArray::<O, usize>::new(0);
		while by > 0 {
			let shamt = cmp::min(<usize as BitMemory>::BITS as usize, by);
			let mid = len - shamt;
			unsafe {
				let tmp_bits = tmp.get_unchecked_mut(.. shamt);
				tmp_bits.clone_from_bitslice(self.get_unchecked(mid ..));
				self.copy_within_unchecked(.. mid, shamt);
				self.get_unchecked_mut(.. shamt)
					.clone_from_bitslice(tmp_bits);
			}
			by -= shamt;
		}
	}

	/// Copies the bits from `src` into `self`.
	///
	/// The length of `src` must be the same as `self`.
	///
	/// If you are attempting to write an integer value into a `BitSlice`, see
	/// the [`BitField::store`] trait function.
	///
	/// # Implementation
	///
	/// This method is by necessity a bit-by-bit individual walk across both
	/// slices. Benchmarks indicate that where the slices share type parameters,
	/// this is very close in performance to an element-wise `memcpy`. You
	/// should use this method as the default transfer behavior, and only switch
	/// to [`.copy_from_bitslice()`] where you know that your performance is an
	/// issue *and* you can demonstrate that `.copy_from_bitslice()` is
	/// meaningfully better.
	///
	/// Where `self` and `src` are not of the same type parameters, crate
	/// benchmarks show a roughly halved runtime performance.
	///
	/// # Original
	///
	/// [`slice::clone_from_slice`](https://doc.rust-lang.org/std/primitive.slice.html#method.clone_from_slice)
	///
	/// # API Differences
	///
	/// This method is renamed, as it takes a bit slice rather than an element
	/// slice.
	///
	/// # Panics
	///
	/// This function will panic if the two slices have different lengths.
	///
	/// # Examples
	///
	/// Cloning two bits from a slice into another:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// let src = 0x0Fu16.view_bits::<Lsb0>();
	/// bits[.. 2].clone_from_bitslice(&src[2 .. 4]);
	/// assert_eq!(data, 0xC0);
	/// ```
	///
	/// Rust enforces that there can only be one mutable reference with no
	/// immutable references to a particular piece of data in a particular
	/// scope. Because of this, attempting to use `clone_from_bitslice` on a
	/// single slice will result in a compile failure:
	///
	/// ```rust,compile_fail
	/// use bitvec::prelude::*;
	///
	/// let mut data = 3u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits[.. 2].clone_from_bitslice(&bits[6 ..]);
	/// ```
	///
	/// To work around this, we can use [`split_at_mut`] to create two distinct
	/// sub-slices from a slice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 3u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// let (head, tail) = bits.split_at_mut(4);
	/// head.clone_from_bitslice(tail);
	/// assert_eq!(data, 0x33);
	/// ```
	///
	/// [`BitField::store`]: ../field/trait.BitField.html#method.store
	/// [`split_at_mut`]: #method.split_at_mut
	#[inline]
	pub fn clone_from_bitslice<O2, T2>(&mut self, src: &BitSlice<O2, T2>)
	where
		O2: BitOrder,
		T2: BitStore,
	{
		let len = self.len();
		assert_eq!(
			len,
			src.len(),
			"Cloning between slices requires equal lengths"
		);
		for idx in 0 .. len {
			unsafe {
				self.set_unchecked(idx, *src.get_unchecked(idx));
			}
		}
	}

	#[doc(hidden)]
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(note = "Use `.clone_from_bitslice` to copy between bitslices")]
	pub fn clone_from_slice<O2, T2>(&mut self, src: &BitSlice<O2, T2>)
	where
		O2: BitOrder,
		T2: BitStore,
	{
		self.clone_from_bitslice(src)
	}

	/// Copies all bits from `src` into `self`.
	///
	/// The length of `src` must be the same as `self`.
	///
	/// If you are attempting to write an integer value into a `BitSlice`, see
	/// the [`BitField::store`] trait function.
	///
	/// # Implementation
	///
	/// This method attempts to use `memcpy` element-wise copy acceleration
	/// where possible. This will only occur when both `src` and `self` are
	/// exactly similar: in addition to having the same type parameters and
	/// length, they must begin at the same offset in an element.
	///
	/// Benchmarks do not indicate that `memcpy` element-wise copy is
	/// significantly faster than [`.clone_from_bitslice()`]’s bit-wise crawl.
	/// This implementation is retained so that you have the ability to observe
	/// performance characteristics on your own targets and choose as
	/// appropriate.
	///
	/// # Original
	///
	/// [`slice::copy_from_slice`](https://doc.rust-lang.org/std/primitive.std.html#method.copy_from_slice)
	///
	/// # API Differences
	///
	/// This method is renamed, as it takes a bit slice rather than an element
	/// slice.
	///
	/// # Panics
	///
	/// This function will panic if the two slices have different lengths.
	///
	/// # Examples
	///
	/// Copying two bits from a slice into another:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut dst = bits![mut 0; 200];
	/// let src = bits![1; 200];
	///
	/// assert!(dst.not_any());
	/// dst.copy_from_bitslice(src);
	/// assert!(dst.all());
	/// ```
	/// [`BitField::store`]: ../field/trait.BitField.html#method.store
	/// [`.clone_from_bitslice()`]: #method.clone_from_bitslice
	#[inline]
	pub fn copy_from_bitslice(&mut self, src: &Self) {
		let len = self.len();
		assert_eq!(
			len,
			src.len(),
			"Copying between slices requires equal lengths"
		);

		let (d_head, s_head) = (self.bitptr().head(), src.bitptr().head());
		if d_head == s_head {
			match (self.domain_mut(), src.domain()) {
				(
					DomainMut::Enclave {
						elem: d_elem, tail, ..
					},
					Domain::Enclave { elem: s_elem, .. },
				) => {
					let mask = O::mask(d_head, tail);
					d_elem.clear_bits(mask);
					d_elem.set_bits(mask & s_elem.load_value());
				},
				(
					DomainMut::Region {
						head: d_head,
						body: d_body,
						tail: d_tail,
					},
					Domain::Region {
						head: s_head,
						body: s_body,
						tail: s_tail,
					},
				) => {
					if let (Some((h_idx, dh_elem)), Some((_, sh_elem))) =
						(d_head, s_head)
					{
						let mask = O::mask(h_idx, None);
						dh_elem.clear_bits(mask);
						dh_elem.set_bits(mask & sh_elem.load_value());
					}
					d_body.copy_from_slice(s_body);
					if let (Some((dt_elem, t_idx)), Some((st_elem, _))) =
						(d_tail, s_tail)
					{
						let mask = O::mask(None, t_idx);
						dt_elem.clear_bits(mask);
						dt_elem.set_bits(mask & st_elem.load_value());
					}
				},
				_ => unreachable!(
					"Slices with equal type parameters, lengths, and heads \
					 will always have equal domains"
				),
			}
		}
		self.clone_from_bitslice(src);
	}

	#[doc(hidden)]
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(note = "Use `.copy_from_bitslice` to copy between bitslices")]
	pub fn copy_from_slice(&mut self, src: &Self) {
		self.copy_from_bitslice(src)
	}

	/// Copies bits from one part of the slice to another part of itself.
	///
	/// `src` is the range within `self` to copy from. `dest` is the starting
	/// index of the range within `self` to copy to, which will have the same
	/// length as `src`. The two ranges may overlap. The ends of the two ranges
	/// must be less than or equal to `self.len()`.
	///
	/// # Original
	///
	/// [`slice::copy_within`](https://doc.rust-lang.org/std/primitive.slice.html#method.copy_within)
	///
	/// # Panics
	///
	/// This function will panic if either range exceeds the end of the slice,
	/// or if the end of `src` is before the start.
	///
	/// # Examples
	///
	/// Copying four bytes within a slice:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0x07u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits.copy_within(5 .., 0);
	/// assert_eq!(data, 0xE7);
	/// ```
	#[inline]
	pub fn copy_within<R>(&mut self, src: R, dest: usize)
	where R: RangeBounds<usize> {
		let len = self.len();
		let src = dvl::normalize_range(src, len);
		//  Check that the source range is within bounds,
		dvl::assert_range(src.clone(), len);
		//  And that the destination range is within bounds.
		dvl::assert_range(dest .. dest + (src.end - src.start), len);
		unsafe {
			self.copy_within_unchecked(src, dest);
		}
	}

	/// Swaps all bits in `self` with those in `other`.
	///
	/// The length of `other` must be the same as `self`.
	///
	/// # Original
	///
	/// [`slice::swap_with_slice`](https://doc.rust-lang.org/std/primitive.slice.html#method.swap_with_slice)
	///
	/// # API Differences
	///
	/// This method is renamed, as it takes a bit slice rather than an element
	/// slice.
	///
	/// # Panics
	///
	/// This function will panic if the two slices have different lengths.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut one = [0xA5u8, 0x69];
	/// let mut two = 0x1234u16;
	/// let one_bits = one.view_bits_mut::<Msb0>();
	/// let two_bits = two.view_bits_mut::<Lsb0>();
	///
	/// one_bits.swap_with_bitslice(two_bits);
	///
	/// assert_eq!(one, [0x2C, 0x48]);
	/// # #[cfg(target_endian = "little")] {
	/// assert_eq!(two, 0x96A5);
	/// # }
	/// ```
	#[inline]
	pub fn swap_with_bitslice<O2, T2>(&mut self, other: &mut BitSlice<O2, T2>)
	where
		O2: BitOrder,
		T2: BitStore,
	{
		let len = self.len();
		assert_eq!(len, other.len());
		for n in 0 .. len {
			unsafe {
				let (this, that) =
					(*self.get_unchecked(n), *other.get_unchecked(n));
				self.set_unchecked(n, that);
				other.set_unchecked(n, this);
			}
		}
	}

	#[doc(hidden)]
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(note = "Use `.swap_with_bitslice` to swap between bitslices")]
	pub fn swap_with_slice<O2, T2>(&mut self, other: &mut BitSlice<O2, T2>)
	where
		O2: BitOrder,
		T2: BitStore,
	{
		self.swap_with_bitslice(other);
	}

	/// Transmute the bitslice to a bitslice of another type, ensuring alignment
	/// of the types is maintained.
	///
	/// This method splits the bitslice into three distinct bitslices: prefix,
	/// correctly aligned middle bitslice of a new type, and the suffix
	/// bitslice. The method may make the middle bitslice the greatest
	/// length possible for a given type and input bitslice, but only your
	/// algorithm's performance should depend on that, not its correctness. It
	/// is permissible for all of the input data to be returned as the prefix or
	/// suffix bitslice.
	///
	/// # Original
	///
	/// [`slice::align_to`](https://doc.rust-lang.org/std/primitive.slice.html#method.align_to)
	///
	/// # API Differences
	///
	/// Type `U` is **required** to have the same type family as type `T`.
	/// Whatever `T` is of the fundamental integers, atomics, or `Cell`
	/// wrappers, `U` must be a different width in the same family. Changing the
	/// type family with this method is **unsound** and strictly forbidden.
	/// Unfortunately, it cannot be guaranteed by this function, so you are
	/// required to abide by this limitation.
	///
	/// # Safety
	///
	/// This method is essentially a `transmute` with respect to the elements in
	/// the returned middle bitslice, so all the usual caveats pertaining to
	/// `transmute::<T, U>` also apply here.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// unsafe {
	///   let bytes: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];
	///   let bits = bytes.view_bits::<LocalBits>();
	///   let (prefix, shorts, suffix) = bits.align_to::<u16>();
	///   match prefix.len() {
	///     0 => {
	///       assert_eq!(shorts, bits[.. 48]);
	///       assert_eq!(suffix, bits[48 ..]);
	///     },
	///     8 => {
	///       assert_eq!(prefix, bits[.. 8]);
	///       assert_eq!(shorts, bits[8 ..]);
	///     },
	///     _ => unreachable!("This case will not occur")
	///   }
	/// }
	/// ```
	#[inline]
	pub unsafe fn align_to<U>(&self) -> (&Self, &BitSlice<O, U>, &Self)
	where U: BitStore {
		let bitptr = self.bitptr();
		let bp_len = bitptr.len();
		let (l, c, r) = bitptr.as_aliased_slice().align_to::<U::Alias>();
		let l_start = bitptr.head().value() as usize;
		let mut l = BitSlice::<O, T>::from_aliased_slice_unchecked(l);
		if l.len() > l_start {
			l = l.get_unchecked(l_start ..);
		}
		let mut c = BitSlice::<O, U>::from_aliased_slice_unchecked(c);
		let c_len = cmp::min(c.len(), bp_len - l.len());
		c = c.get_unchecked(.. c_len);
		let mut r = BitSlice::<O, T>::from_aliased_slice_unchecked(r);
		let r_len = bp_len - l.len() - c.len();
		if r.len() > r_len {
			r = r.get_unchecked(.. r_len);
		}
		(
			l.bitptr()
				.pipe(dvl::remove_bitptr_alias::<T>)
				.to_bitslice_ref(),
			c.bitptr()
				.pipe(dvl::remove_bitptr_alias::<U>)
				.to_bitslice_ref(),
			r.bitptr()
				.pipe(dvl::remove_bitptr_alias::<T>)
				.to_bitslice_ref(),
		)
	}

	/// Transmute the bitslice to a bitslice of another type, ensuring alignment
	/// of the types is maintained.
	///
	/// This method splits the bitslice into three distinct bitslices: prefix,
	/// correctly aligned middle bitslice of a new type, and the suffix
	/// bitslice. The method may make the middle bitslice the greatest
	/// length possible for a given type and input bitslice, but only your
	/// algorithm's performance should depend on that, not its correctness. It
	/// is permissible for all of the input data to be returned as the prefix or
	/// suffix bitslice.
	///
	/// # Original
	///
	/// [`slice::align_to`](https://doc.rust-lang.org/std/primitive.slice.html#method.align_to)
	///
	/// # API Differences
	///
	/// Type `U` is **required** to have the same type family as type `T`.
	/// Whatever `T` is of the fundamental integers, atomics, or `Cell`
	/// wrappers, `U` must be a different width in the same family. Changing the
	/// type family with this method is **unsound** and strictly forbidden.
	/// Unfortunately, it cannot be guaranteed by this function, so you are
	/// required to abide by this limitation.
	///
	/// # Safety
	///
	/// This method is essentially a `transmute` with respect to the elements in
	/// the returned middle bitslice, so all the usual caveats pertaining to
	/// `transmute::<T, U>` also apply here.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// unsafe {
	///   let mut bytes: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];
	///   let bits = bytes.view_bits_mut::<LocalBits>();
	///   let (prefix, shorts, suffix) = bits.align_to_mut::<u16>();
	///   //  same access and behavior as in `align_to`
	/// }
	/// ```
	#[inline]
	pub unsafe fn align_to_mut<U>(
		&mut self,
	) -> (&mut Self, &mut BitSlice<O, U>, &mut Self)
	where U: BitStore {
		let (l, c, r) = self.align_to::<U>();
		(
			l.bitptr().to_bitslice_mut(),
			c.bitptr().to_bitslice_mut(),
			r.bitptr().to_bitslice_mut(),
		)
	}
}

/// These functions only exist when `BitVec` does.
#[cfg(feature = "alloc")]
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Copies `self` into a new `BitVec`.
	///
	/// # Original
	///
	/// [`slice::to_vec`](https://doc.rust-lang.org/std.primitive.html#method.to_vec)
	///
	/// # Examples
	///
	/// ```rust
	/// # #[cfg(feature = "stde")] {
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 1];
	/// let bv = bits.to_bitvec();
	/// assert_eq!(bits, bv);
	/// # }
	/// ```
	#[inline(always)]
	pub fn to_bitvec(&self) -> BitVec<O, T> {
		BitVec::from_bitslice(self)
	}

	#[doc(hidden)]
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(note = "Use `.to_bitvec` to convert a bit slice into a vector")]
	pub fn to_vec(&self) -> BitVec<O, T> {
		self.to_bitvec()
	}

	/// Creates a vector by repeating a slice `n` times.
	///
	/// # Original
	///
	/// [`slice::repeat`](https://doc.rust-lang.org/std/primitive.slice.html#method.repeat)
	///
	/// # Panics
	///
	/// This function will panic if the capacity would overflow.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// assert_eq!(bits![0, 1].repeat(3), bits![0, 1, 0, 1, 0, 1]);
	/// ```
	///
	/// A panic upon overflow:
	///
	/// ```rust,should_panic
	/// use bitvec::prelude::*;
	///
	/// // this will panic at runtime
	/// bits![0, 1].repeat(BitSlice::<LocalBits, usize>::MAX_BITS);
	/// ```
	#[inline]
	pub fn repeat(&self, n: usize) -> BitVec<O, T>
	where
		O: BitOrder,
		T: BitStore,
	{
		let len = self.len();
		let total = len.checked_mul(n).expect("capacity overflow");
		//  The memory has to be initialized before `.copy_from_bitslice` can
		//  write into it.
		let mut out = BitVec::repeat(false, total);
		for span in (0 .. n).map(|rep| rep * len .. (rep + 1) * len) {
			unsafe { out.get_unchecked_mut(span) }.copy_from_bitslice(self);
		}
		unsafe {
			out.set_len(total);
		}
		out
	}

	/* As of 1.44, the `concat` and `join` methods use still-unstable traits to
	govern the collection of multiple subslices into one vector. These are
	possible to copy over and redefine locally, but unless a user asks for it,
	doing so is considered a low priority.
	*/
}

/** Converts a reference to `T` into a bitslice over one element.

# Original

[`slice::from_ref`](https://doc.rust-lang.org/core/slice/fn.from_ref.html)
**/
#[inline(always)]
#[cfg(not(tarpaulin_include))]
pub fn from_ref<O, T>(elem: &T) -> &BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore + BitRegister,
{
	BitSlice::from_element(elem)
}

/** Converts a reference to `T` into a bitslice over one element.

# Original

[`slice::from_mut`](https://doc.rust-lang.org/core/slice/fn.from_mut.html)
**/
#[inline(always)]
#[cfg(not(tarpaulin_include))]
pub fn from_mut<O, T>(elem: &mut T) -> &mut BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore + BitRegister,
{
	BitSlice::from_element_mut(elem)
}

/* NOTE: Crate style is to use block doc comments at the left margin. A bug in
`rustfmt` replaces four spaces at left margin with hard tab, which is incorrect
in comments. Once `rustfmt` is fixed, revert these to block comments.
*/

/** Forms a bitslice from a pointer and a length.

The `len` argument is the number of **elements**, not the number of bits.

# Original

[`slice::from_raw_parts`](https://doc.rust-lang.org/core/slice/fn.from_raw_parts.html)

# Safety

Behavior is undefined if any of the following conditions are violated:

**/
/// - `data` must be [valid] for `len * mem::size_of::<T>()` many bytes, and it
///   must be properly aligned. This means in particular:
///   - The entire memory range of this slice must be contained within a single
///     allocated object! Slices can never span across multiple allocated
///     objects.
///   - `data` must be non-null and aligned even for zero-length slices. The
///     `&BitSlice` pointer encoding requires this porperty to hold. You can
///     obtain a pointer that is usable as `data` for zero-length slices using
///     [`NonNull::dangling()`].
/// - The memory referenced by the returned bitslice must not be mutated for the
///   duration of the lifetime `'a`, except inside an `UnsafeCell`.
/// - The total size `len * T::Mem::BITS` of the slice must be no larger than
///   [`BitSlice::<_, T>::MAX_BITS`].
/**

# Caveat

The lifetime for the returned slice is inferred from its usage. To prevent
accidental misuse, it's suggested to tie the lifetime to whichever source
lifetime is safe in the context, such as by providing a helper function taking
the lifetime of a host value for the slice, or by explicit annotation.

# Examples

```rust
use bitvec::prelude::*;
use bitvec::slice as bv_slice;

let x = 42u8;
let ptr = &x as *const _;
let bits = unsafe {
  bv_slice::from_raw_parts::<LocalBits, u8>(ptr, 1)
};
assert_eq!(bits.count_ones(), 3);
```

[valid]: https://doc.rust-lang.org/core/ptr/index.html#safety
[`BitSlice::<_, T>::MAX_BITS`]: struct.BitSlice.html#associatedconstant.MAX_BITS
[`NonNull::dangling()`]: https://doc.rust-lang.org/core/ptr/struct.NonNull.html#method.dangling
**/
#[inline]
#[cfg(not(tarpaulin_include))]
pub unsafe fn from_raw_parts<'a, O, T>(
	data: *const T,
	len: usize,
) -> &'a BitSlice<O, T>
where
	O: BitOrder,
	T: 'a + BitStore + BitMemory,
{
	super::bits_from_raw_parts(data, 0, len * T::Mem::BITS as usize)
		.unwrap_or_else(|| {
			panic!(
				"Failed to construct `&{}BitSlice` from invalid pointer {:p} \
				 or element count {}",
				"", data, len
			)
		})
}

/**
Performs the same functionality as [`from_raw_parts`], except that a mutable
bitslice is returned.

# Original

[`slice::from_raw_parts_mut`](https://doc.rust-lang.org/core/slice/fn.from_raw_parts_mut.html)

# Safety

Behavior is undefined if any of the following conditions are violated:
**/
///
/// - `data` must be [valid] for `len * mem::size_of::<T>()` many bytes, and it
///   must be properly aligned. This means in particular:
///   - The entire memory range of this slice must be contained within a single
///     allocated object! Slices can never span across multiple allocated
///     objects.
///   - `data` must be non-null and aligned even for zero-length slices. The
///     `&BitSlice` pointer encoding requires this porperty to hold. You can
///     obtain a pointer that is usable as `data` for zero-length slices using
///     [`NonNull::dangling()`].
/// - The memory referenced by the returned bitslice must not be accessed
///   through other pointer (not derived from the return value) for the duration
///   of the lifetime `'a`. Both read and write accesses are forbidden.
/// - The total size `len * T::Mem::BITS` of the slice must be no larger than
///   [`BitSlice::<_, T>::MAX_BITS`].
///
/// [valid]: https://doc.rust-lang.org/core/ptr/index.html#safety
/// [`from_raw_parts`]: fn.from_raw_parts.html
/// [`NonNull::dangling()`]: https://doc.rust-lang.org/core/ptr/struct.NonNull.html#method.dangling
///
/// [`BitSlice::<_, T>::MAX_BITS`]:
/// struct.BitSlice.html#associatedconstant.MAX_BITS
#[inline]
#[cfg(not(tarpaulin_include))]
pub unsafe fn from_raw_parts_mut<'a, O, T>(
	data: *mut T,
	len: usize,
) -> &'a mut BitSlice<O, T>
where
	O: BitOrder,
	T: 'a + BitStore + BitMemory,
{
	super::bits_from_raw_parts_mut(data, 0, len * T::Mem::BITS as usize)
		.unwrap_or_else(|| {
			panic!(
				"Failed to construct `&{}BitSlice` from invalid pointer {:p} \
				 or element count {}",
				"mut ", data, len
			)
		})
}

/** A helper trait used for indexing operations.

This trait has its definition stabilized, but has not stabilized its associated
functions. This means it cannot be implemented outside of the distribution
libraries. *Furthermore*, since `bitvec` cannot create `&mut bool` references,
it is insufficient for `bitvec`’s uses.

There is no tracking issue for `feature(slice_index_methods)`.

# Original

[`slice::SliceIndex`](https://doc.rust-lang.org/stable/core/slice/trait.SliceIndex.html)

# API Differences

`SliceIndex::Output` is not usable here, because the `usize` implementation
cannot produce `&mut bool`. Instead, two output types `Immut` and `Mut` are
defined. The range implementations define these to be the appropriately mutable
`BitSlice` reference; the `usize` implementation defines them to be `&bool` and
the proxy type.
**/
pub trait BitSliceIndex<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// The output type for immutable functions.
	type Immut;

	/// The output type for mutable functions.
	type Mut;

	/// Returns a shared reference to the output at this location, if in bounds.
	///
	/// # Original
	///
	/// [`SliceIndex::get`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.get)
	fn get(self, slice: &'a BitSlice<O, T>) -> Option<Self::Immut>;

	/// Returns a mutable reference to the output at this location, if in
	/// bounds.
	///
	/// # Original
	///
	/// [`SliceIndex::get_mut`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.get_mut)
	fn get_mut(self, slice: &'a mut BitSlice<O, T>) -> Option<Self::Mut>;

	/// Returns a shared reference to the output at this location, without
	/// performing any bounds checking. Calling this method with an
	/// out-of-bounds index is [undefined behavior] even if the resulting
	/// reference is not used.
	///
	/// # Original
	///
	/// [`SliceIndex::get_unchecked`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.get_unchecked)
	///
	/// # Safety
	///
	/// As this function does not perform boundary checking, the caller must
	/// ensure that `self` is an index within the boundaries of `slice` before
	/// calling in order to prevent boundary escapes and the ensuing safety
	/// violations.
	///
	/// [undefined behavior]: https://doc.rust-lang.org/reference/behavior-considered-undefined.html
	unsafe fn get_unchecked(self, slice: &'a BitSlice<O, T>) -> Self::Immut;

	/// Returns a mutable reference to the output at this location, without
	/// performing any bounds checking. Calling this method with an
	/// out-of-bounds index is [undefined behavior] even if the resulting
	/// reference is not used.
	///
	/// # Original
	///
	/// [`SliceIndex::get_unchecked_mut`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.get_unchecked_mut)
	///
	/// # Safety
	///
	/// As this function does not perform boundary checking, the caller must
	/// ensure that `self` is an index within the boundaries of `slice` before
	/// calling in order to prevent boundary escapes and the ensuing safety
	/// violations.
	///
	/// [undefined behavior]: https://doc.rust-lang.org/reference/behavior-considered-undefined.html
	unsafe fn get_unchecked_mut(
		self,
		slice: &'a mut BitSlice<O, T>,
	) -> Self::Mut;

	/// Returns a shared reference to the output at this location, panicking if
	/// out of bounds.
	///
	/// # Original
	///
	/// [`SliceIndex::index`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.index)
	fn index(self, slice: &'a BitSlice<O, T>) -> Self::Immut;

	/// Returns a mutable reference to the output at this location, panicking if
	/// out of bounds.
	///
	/// # Original
	///
	/// [`SliceIndex::index_mut`](https://doc.rust-lang.org/core/slice/trait.SliceIndex.html#method.index_mut)
	fn index_mut(self, slice: &'a mut BitSlice<O, T>) -> Self::Mut;
}

impl<'a, O, T> BitSliceIndex<'a, O, T> for usize
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	type Immut = &'a bool;
	type Mut = BitMut<'a, O, T>;

	#[inline]
	fn get(self, slice: &'a BitSlice<O, T>) -> Option<Self::Immut> {
		if self < slice.len() {
			Some(unsafe { self.get_unchecked(slice) })
		}
		else {
			None
		}
	}

	#[inline]
	fn get_mut(self, slice: &'a mut BitSlice<O, T>) -> Option<Self::Mut> {
		if self < slice.len() {
			Some(unsafe { self.get_unchecked_mut(slice) })
		}
		else {
			None
		}
	}

	#[inline]
	unsafe fn get_unchecked(self, slice: &'a BitSlice<O, T>) -> Self::Immut {
		if slice.bitptr().read::<O>(self) {
			&true
		}
		else {
			&false
		}
	}

	#[inline]
	unsafe fn get_unchecked_mut(
		self,
		slice: &'a mut BitSlice<O, T>,
	) -> Self::Mut
	{
		let bitptr = slice.bitptr();
		let (elt, bit) = bitptr.head().offset(self as isize);
		let addr = bitptr.pointer().to_access().offset(elt);
		BitMut::new_unchecked(addr, bit)
	}

	#[inline]
	fn index(self, slice: &'a BitSlice<O, T>) -> Self::Immut {
		self.get(slice).unwrap_or_else(|| {
			panic!("Index {} out of bounds: {}", self, slice.len())
		})
	}

	#[inline]
	fn index_mut(self, slice: &'a mut BitSlice<O, T>) -> Self::Mut {
		let len = slice.len();
		self.get_mut(slice)
			.unwrap_or_else(|| panic!("Index {} out of bounds: {}", self, len))
	}
}

/// Implement indexing for the different range types.
macro_rules! range_impl {
	( $r:ty { $get:item $unchecked:item } ) => {
		impl<'a, O, T> BitSliceIndex<'a, O, T> for $r
		where O: 'a + BitOrder, T: 'a + BitStore {
			type Immut = &'a BitSlice<O, T>;
			type Mut = &'a mut BitSlice<O, T>;

			#[inline]
			$get

			#[inline]
			fn get_mut(self, slice: Self::Mut) -> Option<Self::Mut> {
				self.get(slice).map(|s| s.bitptr().to_bitslice_mut())
			}

			#[inline]
			$unchecked

			#[inline]
			unsafe fn get_unchecked_mut(self, slice: Self::Mut) -> Self::Mut {
				self.get_unchecked(slice).bitptr().to_bitslice_mut()
			}

			fn index(self, slice: Self::Immut) -> Self::Immut {
				let r = self.clone();
				let l = slice.len();
				self.get(slice)
					.unwrap_or_else(|| {
						panic!("Range {:?} out of bounds: {}", r, l)
					})
			}

			#[inline]
			fn index_mut(self, slice: Self::Mut) -> Self::Mut {
				self.index(slice).bitptr().to_bitslice_mut()
			}
		}
	};

	( $( $r:ty => map $func:expr; )* ) => { $(
		impl<'a, O, T> BitSliceIndex<'a, O, T> for $r
		where O: 'a + BitOrder, T: 'a + BitStore {
			type Immut = &'a BitSlice<O, T>;
			type Mut = &'a mut BitSlice<O, T>;

			#[inline]
			fn get(self, slice: Self::Immut) -> Option<Self::Immut> {
				$func(self).get(slice)
			}

			#[inline]
			fn get_mut(self, slice: Self::Mut) -> Option<Self::Mut> {
				$func(self).get_mut(slice)
			}

			#[inline]
			unsafe fn get_unchecked(self, slice: Self::Immut) -> Self::Immut {
				$func(self).get_unchecked(slice)
			}

			#[inline]
			unsafe fn get_unchecked_mut(self, slice: Self::Mut) -> Self::Mut {
				$func(self).get_unchecked_mut(slice)
			}

			#[inline]
			fn index(self, slice: Self::Immut) -> Self::Immut {
				$func(self).index(slice)
			}

			#[inline]
			fn index_mut(self, slice: Self::Mut) -> Self::Mut {
				$func(self).index_mut(slice)
			}
		}
	)* };
}

range_impl!(Range<usize> {
	fn get(self, slice: Self::Immut) -> Option<Self::Immut> {
		let len = slice.len();

		if self.start > len || self.end > len || self.start > self.end {
			return None;
		}

		Some(unsafe { (self.start .. self.end).get_unchecked(slice) })
	}

	unsafe fn get_unchecked(self, slice: Self::Immut) -> Self::Immut {
		let (addr, head, _) = slice.bitptr().raw_parts();

		let (skip, new_head) = head.offset(self.start as isize);

		BitPtr::new_unchecked(
			addr.to_const().offset(skip),
			new_head,
			self.end - self.start,
		).to_bitslice_ref()
	}
});

range_impl!(RangeFrom<usize> {
	fn get(self, slice: Self::Immut) -> Option<Self::Immut> {
		let len = slice.len();
		if self.start <= len {
			Some(unsafe { (self.start ..).get_unchecked(slice) })
		}
		else {
			None
		}
	}

	unsafe fn get_unchecked(self, slice: Self::Immut) -> Self::Immut {
		let (addr, head, bits) = slice.bitptr().raw_parts();

		let (skip, new_head) = head.offset(self.start as isize);

		BitPtr::new_unchecked(
			addr.to_const().offset(skip),
			new_head,
			bits - self.start,
		).to_bitslice_ref()
	}
});

range_impl!(RangeTo<usize> {
	// `.. end` just changes the length
	fn get(self, slice: Self::Immut) -> Option<Self::Immut> {
		let len = slice.len();
		if self.end <= len {
			Some(unsafe { (.. self.end).get_unchecked(slice) })
		}
		else {
			None
		}
	}

	unsafe fn get_unchecked(self, slice: Self::Immut) -> Self::Immut {
		slice.bitptr().tap_mut(|bp| bp.set_len(self.end)).to_bitslice_ref()
	}
});

range_impl! {
	RangeInclusive<usize> => map |this: Self| {
		#[allow(clippy::range_plus_one)]
		(*this.start() .. *this.end() + 1)
	};

	RangeToInclusive<usize> => map |RangeToInclusive { end }| {
		#[allow(clippy::range_plus_one)]
		(.. end + 1)
	};
}

/// `RangeFull` is the identity function.
#[cfg(not(tarpaulin_include))]
impl<'a, O, T> BitSliceIndex<'a, O, T> for RangeFull
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	type Immut = &'a BitSlice<O, T>;
	type Mut = &'a mut BitSlice<O, T>;

	#[inline(always)]
	fn get(self, slice: Self::Immut) -> Option<Self::Immut> {
		Some(slice)
	}

	#[inline(always)]
	fn get_mut(self, slice: Self::Mut) -> Option<Self::Mut> {
		Some(slice)
	}

	#[inline(always)]
	unsafe fn get_unchecked(self, slice: Self::Immut) -> Self::Immut {
		slice
	}

	#[inline(always)]
	unsafe fn get_unchecked_mut(self, slice: Self::Mut) -> Self::Mut {
		slice
	}

	#[inline(always)]
	fn index(self, slice: Self::Immut) -> Self::Immut {
		slice
	}

	#[inline(always)]
	fn index_mut(self, slice: Self::Mut) -> Self::Mut {
		slice
	}
}
