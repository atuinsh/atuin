//! `BitSlice` iterators

use crate::{
	index::BitIdx,
	mem::BitMemory,
	order::BitOrder,
	pointer::BitPtr,
	slice::{
		proxy::BitMut,
		BitSlice,
		BitSliceIndex,
	},
	store::BitStore,
};

use core::{
	cmp,
	fmt::{
		self,
		Debug,
		Formatter,
	},
	iter::FusedIterator,
	marker::PhantomData,
	mem,
	ptr::NonNull,
};

/** Immutable slice iterator

This struct is created by the [`iter`] method on [`BitSlice`]s.

# Original

[`slice::Iter`](https://doc.rust-lang.org/core/slice/struct.Iter.html)

# Examples

Basic usage:

```rust
# #[cfg(feature = "std")] {
use bitvec::prelude::*;

// First, we declare a type which has `iter` method to get the `Iter` struct (&BitSlice here):
let data = 129u8;
let bits = BitSlice::<LocalBits, _>::from_element(&data);

// Then, we iterato over it:
for bit in bits.iter() {
  println!("{}", bit);
}
# }
```

[`BitSlice`]: struct.BitSlice.html
[`iter`]: struct.BitSlice.html#method.iter
**/
#[derive(Debug)]
pub struct Iter<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// Address of the element with the first live bit.
	base: *const T,
	/// Address of the element containing the first dead bit.
	///
	/// This address may or may not be dereferencable, but thanks to a rule in
	/// the C++ (and thus LLVM) memory model emplaced specifically to allow
	/// double-pointer iteration, creation of an address one element after the
	/// end of a live region is required to be legal. It is not required to be
	/// equal to a numerically-identical base address of a separate adjoining
	/// region, but that is not important here.
	last: *const T,
	/// Semantic index of the first live bit.
	head: BitIdx<T::Mem>,
	/// Semantic index of the first dead bit after the last live bit. This may
	/// be in an element beyond the dereferencable region.
	///
	/// This is not a `BitTail` because reverse iteration requires a valid
	/// index, and the use of a pointer that may point outside the element
	/// region has a smoother codepath than the use of an index that may be
	/// outside the element.
	tail: BitIdx<T::Mem>,
	/// `Iter` is semantically equivalent to a `&BitSlice`.
	_ref: PhantomData<&'a BitSlice<O, T>>,
}

impl<'a, O, T> Iter<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// Views the underlying data as a subslice of the original data.
	///
	/// This has the same lifetime as the original bit slice, and so the
	/// iterator can continue to be used while this exists.
	///
	/// # Original
	///
	/// [`Iter::as_slice`](https://doc.rust-lang.org/core/slice/struct.Iter.html#method.as_slice)
	///
	/// # API Differences
	///
	/// This is renamed, as its return type is not an element slice `&[T]` or
	/// `&[bool]` but a bit slice.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	///
	/// // First, we declare a type which has the `iter` method to get the `Iter`
	/// // struct (&BitSlice here):
	/// let data = 129u8;
	/// let bits = BitSlice::<Msb0, _>::from_element(&data);
	///
	/// // Then, we get the iterator:
	/// let mut iter = bits.iter();
	/// // So if we print what `as_bitslice` returns here, we have "[1, 0, 0, 0, 0, 0, 0, 1]":
	/// println!("{:?}", iter.as_bitslice());
	///
	/// // Next, we move to the second element of the slice:
	/// iter.next();
	/// // Now `as_bitslice` returns "[0, 0, 0, 0, 0, 0, 1]":
	/// println!("{:?}", iter.as_bitslice());
	/// # }
	/// ```
	#[inline]
	pub fn as_bitslice(&self) -> &'a BitSlice<O, T> {
		unsafe { BitPtr::new_unchecked(self.base, self.head, self.len()) }
			.to_bitslice_ref()
	}

	/* Allow the standard-library name to resolve, but instruct the user to
	rename.

	It is important not to use the name `slice` to refer to any `BitSlice`
	regions, and to keep distinct the views of a `BitSlice` from the views of
	the underlying `[T]` storage slice.
	*/
	#[inline]
	#[doc(hidden)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(
		note = "Use `.as_bitslice` on iterators to view the remaining data"
	)]
	pub fn as_slice(&self) -> &'a BitSlice<O, T> {
		self.as_bitslice()
	}

	/// Removes the bit at the front of the iterator.
	fn pop_front(&mut self) -> <Self as Iterator>::Item {
		let out = unsafe { &*self.base }.get_bit::<O>(self.head);
		let (head, incr) = self.head.incr();
		self.base = unsafe { self.base.add(incr as usize) };
		self.head = head;

		if out { &true } else { &false }
	}

	/// Removes the bit at the back of the iterator.
	fn pop_back(&mut self) -> <Self as Iterator>::Item {
		let (tail, offset) = self.tail.decr();
		self.last = unsafe { self.last.offset(-(offset as isize)) };
		self.tail = tail;
		if unsafe { &*self.last }.get_bit::<O>(self.tail) {
			&true
		}
		else {
			&false
		}
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> Clone for Iter<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	fn clone(&self) -> Self {
		*self
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, T> AsRef<BitSlice<O, T>> for Iter<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	fn as_ref(&self) -> &BitSlice<O, T> {
		self.as_bitslice()
	}
}

impl<'a, O, T> IntoIterator for &'a BitSlice<O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	type IntoIter = Iter<'a, O, T>;
	type Item = <Self::IntoIter as Iterator>::Item;

	fn into_iter(self) -> Self::IntoIter {
		let (addr, head, bits) = self.bitptr().raw_parts();

		let base = addr.to_const();

		let (elts, tail) = head.offset(bits as isize);
		let last = unsafe { base.offset(elts) };

		Self::IntoIter {
			base,
			last,
			head,
			tail,
			_ref: PhantomData,
		}
	}
}

impl<O, T> Copy for Iter<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
}

/** Mutable bit slice iterator.

This struct is created by the [`iter_mut`] method on [`BitSlice`]s.

# Original

[`slice::IterMut`](https://doc.rust-lang.org/core/slice/struct.IterMut.html)

# API Differences

In addition to returning `BitMut` instead of `&mut bool`, all references
produced from this iterator are marked as aliasing. This is necessary because
the references receive the lifetime of the original slice, not of the iterator
object, and the iterator is able to produce multiple live references in the same
scope.

# Examples

Basic usage:

```rust
use bitvec::prelude::*;
// First, we declare a type which has `iter_mut` method to get the `IterMut`
// struct (&BitSlice here):
let mut data = 0u8;
let bits = data.view_bits_mut::<Msb0>();

// Then, we iterate over it and modify bits:
for (idx, mut bit) in bits.iter_mut().enumerate() {
  *bit = idx % 3 == 0;
}
assert_eq!(data, 0b100_100_10);
```

[`BitSlice`]: struct.BitSlice.html
[`iter_mut`]: struct.BitSlice.html#method.iter_mut
**/
#[derive(Debug)]
pub struct IterMut<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// Address of the element with the first live bit.
	base: NonNull<<T::Alias as BitStore>::Access>,
	/// Address of the element with the first dead bit. See `Iter.last`.
	last: NonNull<<T::Alias as BitStore>::Access>,
	/// Index of the first live bit in `*base`.
	head: BitIdx<<T::Alias as BitStore>::Mem>,
	/// Index of the first dead bit in `*last`. See `Iter.tail`.
	tail: BitIdx<<T::Alias as BitStore>::Mem>,
	/// `IterMut` is semantically an aliasing `&mut BitSlice`.
	_ref: PhantomData<&'a mut BitSlice<O, T::Alias>>,
}

impl<'a, O, T> IterMut<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// Views the underlying data as a subslice of the original data.
	///
	/// To avoid creating `&mut` references that alias the same *bits*, this is
	/// forced to consume the iterator.
	///
	/// # Original
	///
	/// [`IterMut::into_bitslice`](https://doc.rust-lang.org/core/slice/struct.IterMut.html#method.into_bitslice)
	///
	/// # API Differences
	///
	/// This is renamed, as its return type is not an element slice `&mut [T]`
	/// or `&mut [bool]` but a bit slice.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	///
	/// // First, we declare a type which has `iter_mut` method to get the `IterMut`
	/// // struct (&BitSlice here):
	/// let mut data = 0u8;
	/// let bits = data.view_bits_mut::<Lsb0>();
	///
	/// {
	///   // Then, we get the iterator:
	///   let mut iter = bits.iter_mut();
	///   // We move to the next element:
	///   iter.next();
	///   // So if we print what `into_bitslice` method returns here, we have
	///   // "[0, 0, 0, 0, 0, 0, 0]":
	///   println!("{:?}", iter.into_bitslice());
	/// }
	///
	/// // Now let's modify a value of the slice:
	/// {
	///   // First we get back the iterator:
	///   let mut iter = bits.iter_mut();
	///   // We change the value of the first bit of the slice returned by the `next` method:
	///   *iter.next().unwrap() = true;
	/// }
	/// // Now data is "1":
	/// assert_eq!(data, 1);
	/// # }
	pub fn into_bitslice(self) -> &'a mut BitSlice<O, T::Alias> {
		unsafe {
			BitPtr::new_unchecked(
				self.base.as_ptr()
					as *const <<T as BitStore>::Alias as BitStore>::Access
					as *const <T as BitStore>::Alias,
				self.head,
				self.len(),
			)
		}
		.to_bitslice_mut()
	}

	/* Allow the standard-library name to resolve, but instruct the user to
	rename.

	It is important not to use the name `slice` to refer to any `BitSlice`
	regions, and to keep distinct the views of a `BitSlice` from the views of
	the underlying `[T]` storage slice.
	*/
	#[inline]
	#[doc(hidden)]
	#[cfg(not(tarpaulin_include))]
	#[deprecated(note = "Use `.into_bitslice` on mutable iterators to view \
	                     the remaining data")]
	pub fn into_slice(self) -> &'a mut BitSlice<O, T::Alias> {
		self.into_bitslice()
	}

	/// Removes the bit at the front of the iterator.
	fn pop_front(&mut self) -> <Self as Iterator>::Item {
		let out =
			unsafe { BitMut::new_unchecked(self.base.as_ptr(), self.head) };

		let (head, incr) = self.head.incr();
		self.base = unsafe {
			NonNull::new_unchecked(self.base.as_ptr().add(incr as usize))
		};
		self.head = head;

		out
	}

	/// Removes the bit at the back of the iterator.
	fn pop_back(&mut self) -> <Self as Iterator>::Item {
		let (tail, decr) = self.tail.decr();
		self.last = unsafe {
			NonNull::new_unchecked(self.last.as_ptr().sub(decr as usize))
		};
		self.tail = tail;

		unsafe { BitMut::new_unchecked(self.last.as_ptr(), self.tail) }
	}
}

impl<'a, O, T> IntoIterator for &'a mut BitSlice<O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	type IntoIter = IterMut<'a, O, T>;
	type Item = <Self::IntoIter as Iterator>::Item;

	fn into_iter(self) -> Self::IntoIter {
		let (addr, head, bits) = self.alias().bitptr().raw_parts();

		let addr = addr.to_access()
			as *mut <<T as BitStore>::Alias as BitStore>::Access;
		let base = unsafe { NonNull::new_unchecked(addr) };

		let (elts, tail) = head.offset(bits as isize);
		let last = unsafe { NonNull::new_unchecked(addr.offset(elts)) };

		Self::IntoIter {
			base,
			last,
			head,
			tail,
			_ref: PhantomData,
		}
	}
}

impl<'a, O, T> Iter<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// The canonical empty iterator.
	const EMPTY: Self = Self {
		base: NonNull::dangling().as_ptr() as *const T,
		last: NonNull::dangling().as_ptr() as *const T,
		head: BitIdx::ZERO,
		tail: BitIdx::ZERO,
		_ref: PhantomData,
	};

	#[inline(always)]
	fn get_base(&self) -> *const T {
		self.base
	}

	#[inline(always)]
	fn get_last(&self) -> *const T {
		self.last
	}

	#[inline(always)]
	fn set_base(&mut self, base: *const T) {
		self.base = base
	}

	#[inline(always)]
	fn set_last(&mut self, last: *const T) {
		self.last = last
	}
}

impl<'a, O, T> IterMut<'a, O, T>
where
	O: 'a + BitOrder,
	T: 'a + BitStore,
{
	/// The canonical empty iterator.
	const EMPTY: Self = Self {
		base: NonNull::dangling(),
		last: NonNull::dangling(),
		head: BitIdx::ZERO,
		tail: BitIdx::ZERO,
		_ref: PhantomData,
	};

	#[inline(always)]
	fn get_base(&self) -> *mut <T::Alias as BitStore>::Access {
		self.base.as_ptr()
	}

	#[inline(always)]
	fn get_last(&self) -> *mut <T::Alias as BitStore>::Access {
		self.last.as_ptr()
	}

	#[inline(always)]
	fn set_base(&mut self, base: *mut <T::Alias as BitStore>::Access) {
		self.base = unsafe { NonNull::new_unchecked(base) }
	}

	#[inline(always)]
	fn set_last(&mut self, last: *mut <T::Alias as BitStore>::Access) {
		self.last = unsafe { NonNull::new_unchecked(last) }
	}
}

/// `Iter` and `IterMut` have very nearly the same implementation text.
macro_rules! iter {
	($($t:ident => $i:ty),+ $(,)?) => { $(
		impl<'a, O, T> $t<'a, O, T>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
		{
			/// Tests whether the iterator is *any* empty iterator.
			pub(crate) fn inherent_is_empty(&self) -> bool {
				self.base == self.last && self.head == self.tail
			}
		}

		impl<'a, O, T> Iterator for $t<'a, O, T>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
		{
			type Item = $i;

			#[inline]
			fn next(&mut self) -> Option<Self::Item> {
				if self.inherent_is_empty() {
					return None;
				}
				Some(self.pop_front())
			}

			#[inline]
			fn size_hint(&self) -> (usize, Option<usize>) {
				let len = self.len();
				(len, Some(len))
			}

			#[inline]
			fn count(self) -> usize {
				self.len()
			}

			#[inline]
			fn nth(&mut self, n: usize) -> Option<Self::Item> {
				if n >= self.len() {
					*self = Self::EMPTY;
					return None;
				}

				//  Move the head cursors up by `n` bits before producing a bit.
				let (elts, head) = self.head.offset(n as isize);
				self.set_base(unsafe{self.get_base().offset(elts)});
				self.head = head;
				Some(self.pop_front())
			}

			#[inline]
			fn last(mut self) -> Option<Self::Item> {
				self.next_back()
			}
		}

		impl<'a, O, T> DoubleEndedIterator for $t <'a, O, T>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
		{
			#[inline]
			fn next_back(&mut self) -> Option<Self::Item> {
				if self.inherent_is_empty() {
					return None;
				}
				Some(self.pop_back())
			}

			#[inline]
			fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
				if n >= self.len() {
					*self = Self::EMPTY;
					return None;
				}

				//  Move the tail cursors down by `n` bits before producing a
				//  bit.
				let (elts, tail) = self.tail.offset(-(n as isize));
				self.set_last(unsafe{self.get_last().offset(elts)});
				self.tail = tail;
				Some(self.pop_back())
			}
		}

		impl<O, T> ExactSizeIterator for $t <'_, O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
			fn len(&self) -> usize {
				let (base, last) =
					(self.get_base() as usize, self.get_last() as usize);
				/* Get the total number of bits in the element range
				`self.base .. self.last`. Wrapping arithmetic is used because
				`last` is known to never be less than base, so we always want a
				bare `sub` instruction without any checks. We also know that the
				difference between the two addresses can support a `shl`
				instruction without overflow.
				*/
				last.wrapping_sub(base)
					//  Pointers are always byte-stepped, not element-stepped.
					.wrapping_shl(<u8 as BitMemory>::INDX as u32)
					//  Now, add the live bits before `self.tail` in `*last`,
					.wrapping_add(self.tail.value() as usize)
					//  And remove the dead bits before `self.head` in `*base`.
					.wrapping_sub(self.head.value() as usize)
			}
		}

		impl<O, T> FusedIterator for $t <'_, O, T>
		where
			O: BitOrder,
			T: BitStore
		{
		}

		unsafe impl<O, T> Send for $t <'_, O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
		}

		unsafe impl<O, T> Sync for $t <'_, O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
		}
	)+ };
}

iter!(
	Iter => <usize as BitSliceIndex<'a, O, T>>::Immut,
	IterMut => <usize as BitSliceIndex<'a, O, T::Alias>>::Mut,
);

/// Creates a full iterator set from only the base functions needed to build it.
macro_rules! group {
	(
		//  The type for the iteration set. This must be an immutable group.
		$iter:ident => $item:ty $( where $alias:ident )? {
			//  The eponymous functions from the iterator traits.
			$next:item
			$nth:item
			$next_back:item
			$nth_back:item
			$len:item
		}
	) => {
		//  Immutable iterator implementation
		impl<'a, O, T> Iterator for $iter <'a, O, T>
		where
			O: BitOrder,
			T: 'a + BitStore,
		{
			type Item = $item;

			#[inline]
			$next

			#[inline]
			$nth

			#[inline]
			fn size_hint(&self) -> (usize, Option<usize>) {
				let len = self.len();
				(len, Some(len))
			}

			#[inline]
			fn count(self) -> usize {
				self.len()
			}

			#[inline]
			fn last(mut self) -> Option<Self::Item> {
				self.next_back()
			}
		}

		impl<'a, O, T> DoubleEndedIterator for $iter <'a, O, T>
		where
			O: BitOrder,
			T: 'a + BitStore,
		{
			#[inline]
			$next_back

			#[inline]
			$nth_back
		}

		impl<O, T> ExactSizeIterator for $iter <'_, O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
			#[inline]
			$len
		}

		impl<O, T> FusedIterator for $iter <'_, O, T>
		where
			O: BitOrder,
			T: BitStore,
		{
		}
	}
}

/** An iterator over overlapping subslices of length `size`.

This struct is created by the [`windows`] method on [bit slices].

# Original

[`slice::Windows`](https://doc.rust-lang.org/core/slice/struct.Windows.html)

[bit slices]: struct.BitSlice.html
[`windows`]: struct.BitSlice.html#method.windows
**/
#[derive(Clone, Debug)]
pub struct Windows<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being windowed.
	slice: &'a BitSlice<O, T>,
	/// The width of the produced windows.
	width: usize,
}

group!(Windows => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		if self.width > self.slice.len() {
			self.slice = Default::default();
			return None;
		}
		unsafe {
			let out = self.slice.get_unchecked(.. self.width);
			self.slice = self.slice.get_unchecked(1 ..);
			Some(out)
		}
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let (end, ovf) = self.width.overflowing_add(n);
		if end > self.slice.len() || ovf {
			self.slice = Default::default();
			return None;
		}
		unsafe {
			let out = self.slice.get_unchecked(n .. end);
			self.slice = self.slice.get_unchecked(n + 1 ..);
			Some(out)
		}
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let len = self.slice.len();
		if self.width > len {
			self.slice = Default::default();
			return None;
		}
		unsafe {
			let out = self.slice.get_unchecked(len - self.width ..);
			self.slice = self.slice.get_unchecked(.. len - 1);
			Some(out)
		}
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let (end, ovf) = self.slice.len().overflowing_sub(n);
		if end < self.width || ovf {
			self.slice = Default::default();
			return None;
		}
		unsafe {
			let out = self.slice.get_unchecked(end - self.width .. end);
			self.slice = self.slice.get_unchecked(.. end - 1);
			Some(out)
		}
	}

	fn len(&self) -> usize {
		let len = self.slice.len();
		if self.width > len {
			return 0;
		}
		len - self.width + 1
	}
});

/** An iterator over a bit slice in (non-overlapping) chunks (`chunk_size` bits
at a time), starting at the beginning of the slice.

When the slice length is not evenly divided by the chunk size, the last slice of
the iteration will be the remainder.

This struct is created by the [`chunks`] method on [bit slices].

# Original

[`slice::Chunks`](https://doc.rust-lang.org/core/slice/struct.Chunks.html)

[bit slices]: struct.BitSlice.html
[`chunks`]: struct.BitSlice.html#method.chunks
**/
#[derive(Clone, Debug)]
pub struct Chunks<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a BitSlice<O, T>,
	/// The width of the produced chunks.
	width: usize,
}

group!(Chunks => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		let len = self.slice.len();
		if len == 0 {
			return None;
		}
		let mid = cmp::min(len, self.width);
		let (out, rest) = unsafe { self.slice.split_at_unchecked(mid) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.slice.len();
		let (start, ovf) = n.overflowing_mul(self.width);
		if start >= len || ovf {
			self.slice = Default::default();
			return None;
		}
		let (out, rest) = unsafe {
			self.slice
				//  Discard the skipped front chunks,
				.get_unchecked(start ..)
				//  then split at the chunk width, or remnant length.
				.split_at_unchecked(cmp::min(len, self.width))
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		match self.slice.len() {
			0 => None,
			len => {
				//  Determine if the back chunk is a remnant or a whole chunk.
				let rem = len % self.width;
				let size = if rem == 0 { self.width } else { rem };
				let (rest, out) =
					unsafe { self.slice.split_at_unchecked(len - size) };
				self.slice = rest;
				Some(out)
			},
		}
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		if n >= len {
			self.slice = Default::default();
			return None;
		}
		let start = (len - 1 - n) * self.width;
		let width = cmp::min(start + self.width, self.slice.len());
		let (rest, out) = unsafe {
			self.slice
				//  Truncate to the end of the returned chunk,
				.get_unchecked(.. start + width)
				//  then split at the start of the returned chunk.
				.split_at_unchecked(start)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		match self.slice.len() {
			0 => 0,
			len => {
				//  an explicit `div_mod` would be nice here
				let (n, r) = (len / self.width, len % self.width);
				n + (r > 0) as usize
			},
		}
	}
});

/** An iterator over a bit slice in (non-overlapping) mutable chunks
(`chunk_size` bits at a time), starting at the beginning of the slice.

When the slice len is not evenly divided by the chunk size, the last slice of
the iteration will be the remainder.

This struct is created by the [`chunks_mut`] method on [bit slices].

# Original

[`slice::ChunksMut`](https://doc.rust-lang.org/core/slice/struct.ChunksMut.html)

# API Differences

All slices yielded from this iterator are marked as aliased.

[bit slices]: struct.BitSlice.html
[`chunks_mut`]: struct.BitSlice.html#chunks_mut
**/
#[derive(Debug)]
pub struct ChunksMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a mut BitSlice<O, T::Alias>,
	/// The width of the produced chunks.
	width: usize,
}

group!(ChunksMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		if len == 0 {
			return None;
		}
		let mid = cmp::min(len, self.width);
		let (out, rest) = unsafe { slice.split_at_unchecked_mut_noalias(mid) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		let (start, ovf) = n.overflowing_mul(self.width);
		if start >= len || ovf {
			return None;
		}
		let (out, rest) = unsafe {
			slice
				//  Discard the skipped front chunks,
				.get_unchecked_mut(start ..)
				//  then split at the chunk width, or remnant length.
				.split_at_unchecked_mut_noalias(cmp::min(len, self.width))
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		match slice.len() {
			0 => None,
			len => {
				//  Determine if the back chunk is a remnant or a whole chunk.
				let rem = len % self.width;
				let size = if rem == 0 { self.width } else { rem };
				let (rest, out) =
					unsafe { slice.split_at_unchecked_mut_noalias(len - size) };
				self.slice = rest;
				Some(out)
			},
		}
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		let slice = mem::take(&mut self.slice);
		if n >= len {
			return None;
		}
		let start = (len - 1 - n) * self.width;
		let width = cmp::min(start + self.width, slice.len());
		let (rest, out) = unsafe {
			slice
				//  Truncate to the end of the returned chunk,
				.get_unchecked_mut(.. start + width)
				//  then split at the start of the returned chunk.
				.split_at_unchecked_mut_noalias(start)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		match self.slice.len() {
			0 => 0,
			len => {
				//  an explicit `div_mod` would be nice here
				let (n, r) = (len / self.width, len % self.width);
				n + (r > 0) as usize
			},
		}
	}
});

/** An iterator over a bit slice in (non-overlapping) chunks (`chunk_size` bits
at a time), starting at the beginning of the slice.

When the slice len is not evenly divided by the chunk size, the last up to
`chunk_size - 1` bits will be ommitted but can be retrieved from the
[`remainder`] function from the iterator.

This struct is created by the [`chunks_exact`] method on [bit slices].

# Original

[`slice::ChunksExact`](https://doc.rust-lang.org/core/slice/struct.ChunksExact.html)

[bit slices]: struct.BitSlice.html
[`chunks_exact`]: struct.BitSlice.html#method.chunks_exact
[`remainder`]: #method.remainder
**/
#[derive(Clone, Debug)]
pub struct ChunksExact<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a BitSlice<O, T>,
	/// Any remnant of the chunked `BitSlice` not divisible by `width`.
	extra: &'a BitSlice<O, T>,
	/// The width of the produced chunks.
	width: usize,
}

impl<'a, O, T> ChunksExact<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub(super) fn new(slice: &'a BitSlice<O, T>, width: usize) -> Self {
		let len = slice.len();
		let rem = len % width;
		let (slice, extra) = unsafe { slice.split_at_unchecked(len - rem) };
		Self {
			slice,
			extra,
			width,
		}
	}

	/// Returns the remainder of the original bit slice that is not going to be
	/// returned by the iterator. The returned slice has at most `chunk_size-1`
	/// bits.
	///
	/// # Original
	///
	/// [`slice::ChunksExact::remainder`](https://doc.rust-lang.org/core/slice/struct.ChunksExact.html#method.remainder)
	pub fn remainder(&self) -> &'a BitSlice<O, T> {
		self.extra
	}
}

group!(ChunksExact => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		if self.slice.len() < self.width {
			return None;
		}
		let (out, rest) = unsafe { self.slice.split_at_unchecked(self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let (start, ovf) = n.overflowing_mul(self.width);
		if start + self.width >= self.slice.len() || ovf {
			self.slice = Default::default();
			return None;
		}
		let (out, rest) = unsafe {
			self.slice
				.get_unchecked(start ..)
				.split_at_unchecked(self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let len = self.slice.len();
		if len < self.width {
			return None;
		}
		let (rest, out) =
			unsafe { self.slice.split_at_unchecked(len - self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		if n >= len {
			self.slice = Default::default();
			return None;
		}
		let end = (len - n) * self.width;
		let (rest, out) = unsafe {
			self.slice
				.get_unchecked(.. end)
				.split_at_unchecked(end - self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		self.slice.len() / self.width
	}
});

/** An iterator over a bit slice in (non-overlapping) mutable chunks
(`chunk_size` bits at a time), starting at the beginning of the slice.

When the slice len is not evenly divided by the chunk size, the last up to
`chunk_size-1` bits will be omitted but can be retrieved from the
[`into_remainder`] function from the iterator.

This struct is created by the [`chunks_exact_mut`] method on [bit slices].

# Original

[`slice::ChunksExactMut`](https://doc.rust-lang.org/core/slice/struct.ChunksExactMut.html)

# API Differences

All slices yielded from this iterator are marked as aliased.

[bit slices]: struct.BitSlice.html
[`chunks_exact_mut`]: struct.BitSlice.html#method.chunks_exact_mut
[`into_remainder`]: #method.into_remainder
**/
#[derive(Debug)]
pub struct ChunksExactMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a mut BitSlice<O, T::Alias>,
	/// Any remnant of the chunked `BitSlice` not divisible by `width`.
	extra: &'a mut BitSlice<O, T::Alias>,
	/// The width of the produced chunks.
	width: usize,
}

impl<'a, O, T> ChunksExactMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub(super) fn new(slice: &'a mut BitSlice<O, T>, width: usize) -> Self {
		let len = slice.len();
		let rem = len % width;
		let (slice, extra) = unsafe { slice.split_at_unchecked_mut(len - rem) };
		Self {
			slice,
			extra,
			width,
		}
	}

	/// Returns the remainder of the original slice that is not going to be
	/// returned by the iterator. The returned slice has at most `chunk_size-1`
	/// bits.
	///
	/// # Original
	///
	/// [`slice::ChunksExactMut::into_remainder`](https://doc.rust-lang.org/core/slice/struct.ChunksExactMut.html#method.into_remainder)
	///
	/// # API Differences
	///
	/// The remainder slice, as with all slices yielded from this iterator, is
	/// marked as aliased.
	#[inline]
	pub fn into_remainder(self) -> &'a mut BitSlice<O, T::Alias> {
		self.extra
	}
}

group!(ChunksExactMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		if slice.len() < self.width {
			return None;
		}
		let (out, rest) =
			unsafe { slice.split_at_unchecked_mut_noalias(self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let (start, ovf) = n.overflowing_mul(self.width);
		if start + self.width >= slice.len() || ovf {
			return None;
		}
		let (out, rest) = unsafe {
			slice.get_unchecked_mut(start ..)
				.split_at_unchecked_mut_noalias(self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		if len < self.width {
			return None;
		}
		let (rest, out) =
			unsafe { slice.split_at_unchecked_mut_noalias(len - self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		let slice = mem::take(&mut self.slice);
		if n >= len {
			return None;
		}
		let end = (len - n) * self.width;
		let (rest, out) = unsafe {
			slice.get_unchecked_mut(.. end)
				.split_at_unchecked_mut_noalias(end - self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		self.slice.len() / self.width
	}
});

/** An iterator over a bit slice in (non-overlapping) chunks (`chunk_size` bits
at a time), starting at the end of the slice.

When the slice length is not evenly divided by the chunk size, the last slice of
the iteration will be the remainder.

This struct is created by the [`rchunks`] method on [`BitSlice`]s.

# Original

[`slice::RChunks`](https://doc.rust-lang.org/core/slice/struct.RChunks.html)

[`BitSlice`]: struct.BitSlice.html
[`rchunks`]: struct.BitSlice.html#method.rchunks
**/
#[derive(Clone, Debug)]
pub struct RChunks<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a BitSlice<O, T>,
	/// The width of the produced chunks.
	width: usize,
}

group!(RChunks => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		let len = self.slice.len();
		if len == 0 {
			return None;
		}
		let mid = len - cmp::min(len, self.width);
		let (rest, out) = unsafe { self.slice.split_at_unchecked(mid) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.slice.len();
		let (num, ovf) = n.overflowing_mul(self.width);
		if num >= len || ovf {
			self.slice = Default::default();
			return None;
		}
		let end = len - num;
		//  Find the partition between `[.. retain]` and `[return ..][..w]`
		let mid = end.saturating_sub(self.width);
		let (rest, out) = unsafe {
			self.slice
				.get_unchecked(.. end)
				.split_at_unchecked(mid)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		match self.slice.len() {
			0 => None,
			n => {
				let rem = n % self.width;
				let len = if rem == 0 { self.width } else { rem };
				let (out, rest) = unsafe { self.slice.split_at_unchecked(len) };
				self.slice = rest;
				Some(out)
			},
		}
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		if n >= len {
			self.slice = Default::default();
			return None;
		}
		/* Taking from the back of a reverse iterator means taking from the
		front of the slice.

		`len` gives us the total number of subslices remaining. In order to find
		the partition point, we need to subtract `n - 1` full subslices from
		that count (because the back slice of the iteration might not be full),
		compute their bit width, and offset *that* from the end of the memory
		region. This gives us the zero-based index of the partition point
		between what is returned and what is retained.

		The `part ..` section of the slice is retained, and the very end of the
		`.. part` section is returned. The head section is split at no less than
		`self.width` bits below the end marker (this could be the partial
		section, so a wrapping subtraction cannot be used), and `.. start` is
		discarded.

		Source:
		https://doc.rust-lang.org/1.43.0/src/core/slice/mod.rs.html#5141-5156
		*/
		let from_end = (len - 1 - n) * self.width;
		let end = self.slice.len() - from_end;
		let start = end.saturating_sub(self.width);
		let (out, rest) = unsafe { self.slice.split_at_unchecked(end) };
		self.slice = rest;
		Some(unsafe { out.get_unchecked(start ..) })
	}

	fn len(&self) -> usize {
		match self.slice.len() {
			0 => 0,
			len => {
				let (n, r) = (len / self.width, len % self.width);
				n + (r > 0) as usize
			},
		}
	}
});

/** An iterator over a slice in (non-overlapping) mutable chunks (`chunk_size`
bits at a time), starting at the end of the slice.

When the slice length is not evenly divided by the chunk size, the last slice of
the iteration will be the remainder.

This struct is created by the [`rchunks_mut`] method on [bit slices].

# API Differences

All slices yielded from this iterator are marked as aliased.

[bit slices]: struct.BitSlice.html
[`rchunks_mut`]: struct.BitSlice.html#method.rchunks_mut
**/
#[derive(Debug)]
pub struct RChunksMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a mut BitSlice<O, T::Alias>,
	/// The width of the produced chunks.
	width: usize,
}

group!(RChunksMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		if len == 0 {
			return None;
		}
		let mid = len - cmp::min(len, self.width);
		let (rest, out) = unsafe { slice.split_at_unchecked_mut_noalias(mid) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		let (num, ovf) = n.overflowing_mul(self.width);
		if num >= len || ovf {
			return None;
		}
		let end = len - num;
		//  Find the partition between `[.. retain]` and `[return ..][..w]`
		let mid = end.saturating_sub(self.width);
		let (rest, out) = unsafe {
			slice.get_unchecked_mut(.. end)
				.split_at_unchecked_mut_noalias(mid)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		match slice.len() {
			0 => None,
			n => {
				let rem = n % self.width;
				let len = if rem == 0 { self.width } else { rem };
				let (out, rest) =
					unsafe { slice.split_at_unchecked_mut_noalias(len) };
				self.slice = rest;
				Some(out)
			},
		}
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.len();
		let slice = mem::take(&mut self.slice);
		if n >= len {
			return None;
		}
		let from_end = (len - 1 - n) * self.width;
		let end = slice.len() - from_end;
		let start = end.saturating_sub(self.width);
		let (out, rest) = unsafe { slice.split_at_unchecked_mut_noalias(end) };
		self.slice = rest;
		Some(unsafe { out.get_unchecked_mut(start ..) })
	}

	fn len(&self) -> usize {
		match self.slice.len() {
			0 => 0,
			len => {
				let (n, r) = (len / self.width, len % self.width);
				n + (r > 0) as usize
			},
		}
	}
});

/** An iterator over a bit slice in (non-overlapping) chunks (`chunk_size` bits
at a time), starting at the end of the slice.

When the slice len is not evenly divided by the chunk size, the last up to
`chunk_size-1` bits will be omitted but can be retrieved from the [`remainder`]
function from the iterator.

This struct is created by the [`rchunks_exact`] method on [bit slices].

# Original

[`slice::RChunksExact`](https://doc.rust-lang.org/core/slice/struct.RChunksExact.html)

[bit slices]: struct.BitSlice.html
[`rchunks_exact`]: struct.BitSlice.html#method.rchunks_exact
[`remainder`]: #method.remainder
**/
#[derive(Clone, Debug)]
pub struct RChunksExact<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a BitSlice<O, T>,
	/// Any remnant of the chunked `BitSlice` not divisible by `width`.
	extra: &'a BitSlice<O, T>,
	/// The width of the produced chunks.
	width: usize,
}

impl<'a, O, T> RChunksExact<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub(super) fn new(slice: &'a BitSlice<O, T>, width: usize) -> Self {
		let (extra, slice) =
			unsafe { slice.split_at_unchecked(slice.len() % width) };
		Self {
			slice,
			extra,
			width,
		}
	}

	/// Returns the remainder of the original slice that is not going to be
	/// returned by the iterator. The returned slice has at most `chunk_size-1`
	/// bits.
	///
	/// # Original
	///
	/// [`slice::RChunksExact::remainder`](https://doc.rust-lang.org/core/slice/struct.RChunksExact.html#method.remainder)
	#[inline]
	pub fn remainder(&self) -> &'a BitSlice<O, T> {
		self.extra
	}
}

group!(RChunksExact => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		let len = self.slice.len();
		if len < self.width {
			return None;
		}
		let (rest, out) =
			unsafe { self.slice.split_at_unchecked(len - self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.slice.len();
		let (split, ovf) = n.overflowing_mul(self.width);
		if split >= len || ovf {
			self.slice = Default::default();
			return None;
		}
		let end = len - split;
		let (rest, out) = unsafe {
			self.slice
				.get_unchecked(.. end)
				.split_at_unchecked(end - self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		if self.slice.len() < self.width {
			return None;
		}
		let (out, rest) = unsafe { self.slice.split_at_unchecked(self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let len = self.slice.len();
		let (start, ovf) = n.overflowing_mul(self.width);
		if start >= len || ovf {
			self.slice = Default::default();
			return None;
		}
		//  At this point, `start` is at least `self.width` less than `len`.
		let (out, rest) = unsafe {
			self.slice.get_unchecked(start ..).split_at_unchecked(self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		self.slice.len() / self.width
	}
});

/** An iterator over a bit slice in (non-overlapping) mutable chunks
(`chunk_size` bits at a time), starting at the end of the slice.

When the slice len is not evenly divided by the chunk size, the last up to
`chunk_size-1` bits will be omitted but can be retrieved from the
[`into_remainder`] function from the iterator.

This struct is created by the [`rchunks_exact_mut`] method on [bit slices].

# Original

[`slice::RChunksExactMut`](https://doc.rust-lang.org/core/slice/struct.RChunksExactMut.html)

# API Differences

All slices yielded from this iterator are marked as aliased.

[bit slices]: struct.BitSlice.html
[`into_remainder`]: #method.into_remainder
[`rchunks_exact_mut`]: struct.BitSlice.html#method.rchunks_exact_mut
**/
#[derive(Debug)]
pub struct RChunksExactMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// The `BitSlice` being chunked.
	slice: &'a mut BitSlice<O, T::Alias>,
	/// Any remnant of the chunked `BitSlice` not divisible by `width`.
	extra: &'a mut BitSlice<O, T::Alias>,
	/// The width of the produced chunks.
	width: usize,
}

impl<'a, O, T> RChunksExactMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub(super) fn new(slice: &'a mut BitSlice<O, T>, width: usize) -> Self {
		let (extra, slice) =
			unsafe { slice.split_at_unchecked_mut(slice.len() % width) };
		Self {
			slice,
			extra,
			width,
		}
	}

	/// Returns the remainder of the original slice that is not going to be
	/// returned by the iterator. The returned slice has at most `chunk_size-1`
	/// bits.
	///
	/// # Original
	///
	/// [`slice::RChunksExactMut::into_remainder`](https://doc.rust-lang.org/core/slice/struct.RChunksExactMut.html#method.into_remainder)
	///
	/// # API Differences
	///
	/// The remainder slice, as with all slices yielded from this iterator, is
	/// marked as aliased.
	#[inline]
	pub fn into_remainder(self) -> &'a mut BitSlice<O, T::Alias> {
		self.extra
	}
}

group!(RChunksExactMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		if len < self.width {
			return None;
		}
		let (rest, out) =
			unsafe { slice.split_at_unchecked_mut_noalias(len - self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth(&mut self, n: usize) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		let (split, ovf) = n.overflowing_mul(self.width);
		if split >= len || ovf {
			return None;
		}
		let end = len - split;
		let (rest, out) = unsafe {
			slice.get_unchecked_mut(.. end)
				.split_at_unchecked_mut_noalias(end - self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		if slice.len() < self.width {
			return None;
		}
		let (out, rest) =
			unsafe { slice.split_at_unchecked_mut_noalias(self.width) };
		self.slice = rest;
		Some(out)
	}

	fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
		let slice = mem::take(&mut self.slice);
		let len = slice.len();
		let (start, ovf) = n.overflowing_mul(self.width);
		if start >= len || ovf {
			return None;
		}
		//  At this point, `start` is at least `self.width` less than `len`.
		let (out, rest) = unsafe {
			slice.get_unchecked_mut(start ..)
				.split_at_unchecked_mut_noalias(self.width)
		};
		self.slice = rest;
		Some(out)
	}

	fn len(&self) -> usize {
		self.slice.len() / self.width
	}
});

macro_rules! new_group {
	($($t:ident $($m:ident)? $( . $a:ident ())?),+ $(,)?) => { $(
		impl<'a, O, T> $t <'a, O, T>
		where
			O: BitOrder,
			T: BitStore
		{
			#[cfg_attr(not(tarpaulin), inline(always))]
			#[allow(clippy::redundant_field_names)]
			pub(super) fn new(
				slice: &'a $($m)? BitSlice<O, T>,
				width: usize,
			) -> Self {
				Self { slice: slice $( . $a () )?, width }
			}
		}
	)+ };
}

new_group!(
	Windows,
	Chunks,
	ChunksMut mut .alias_mut(),
	RChunks,
	RChunksMut mut .alias_mut(),
);

macro_rules! split {
	($iter:ident => $item:ty $( where $alias:ident )? {
		$next:item
		$next_back:item
	}) => {
		impl<'a, O, T, P> $iter <'a, O, T, P>
		where
			O: BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			#[inline]
			pub(super) fn new(slice: $item, pred: P) -> Self {
				Self {
					slice,
					pred,
					done: false,
				}
			}
		}

		impl<O, T, P> Debug for $iter <'_, O, T, P>
		where
			O: BitOrder,
			T: BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				fmt.debug_struct(stringify!($iter))
					.field("slice", &self.slice)
					.field("done", &self.done)
					.finish()
			}
		}

		impl<'a, O, T, P> Iterator for $iter <'a, O, T, P>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			type Item = $item;

			#[inline]
			$next

			#[inline]
			fn size_hint(&self) -> (usize, Option<usize>) {
				if self.done {
					(0, Some(0))
				}
				else {
					(1, Some(self.slice.len() + 1))
				}
			}
		}

		impl<'a, O, T, P> DoubleEndedIterator for $iter <'a, O, T, P>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			#[inline]
			$next_back
		}

		impl<'a, O, T, P> core::iter::FusedIterator for $iter <'a, O, T, P>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
		}

		impl<'a, O, T, P> SplitIter for $iter <'a, O, T, P>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			#[inline]
			fn finish(&mut self) -> Option<Self::Item> {
				if self.done {
					None
				}
				else {
					self.done = true;
					Some(mem::take(&mut self.slice))
				}
			}
		}
	};
}

/** An iterator over subslices separated by bits that match a predicate
function.

This struct is created by the [`split`] method on [bit slices].

# Original

[`slice::Split`](https://doc.rust-lang.org/core/slice/struct.Split.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`split`]: struct.BitSlice.html#method.split
**/
#[derive(Clone)]
pub struct Split<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	slice: &'a BitSlice<O, T>,
	/// The function used to test whether a split should occur.
	pred: P,
	/// Whether the split is finished.
	done: bool,
}

split!(Split => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		if self.done {
			return None;
		}
		match self.slice
			.iter()
			.enumerate()
			.position(|(idx, bit)| (self.pred)(idx, bit))
		{
			None => self.finish(),
			Some(idx) => unsafe {
				let out = self.slice.get_unchecked(.. idx);
				self.slice = self.slice.get_unchecked(idx + 1 ..);
				Some(out)
			},
		}
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		if self.done {
			return None;
		}
		match self.slice
			.iter()
			.enumerate()
			.rposition(|(idx, bit)| (self.pred)(idx, bit))
		{
			None => self.finish(),
			Some(idx) => unsafe {
				let out = self.slice.get_unchecked(idx + 1 ..);
				self.slice = self.slice.get_unchecked(.. idx);
				Some(out)
			},
		}
	}
});

/** An iterator over the mutable subslices of the slice which are separated by
bits that match `pred`.

This struct is created by the [`split_mut`] method on [bit slices].

# Original

[`slice::SplitMut`](https://doc.rust-lang.org/core/slice/struct.SplitMut.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`split_mut`]: struct.BitSlice.html#method.split_mut
**/
pub struct SplitMut<'a, O, T, P>
where
	O: BitOrder,
	T: BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	slice: &'a mut BitSlice<O, T::Alias>,
	pred: P,
	done: bool,
}

split!(SplitMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		if self.done {
			return None;
		}
		let idx_opt = {
			let pred = &mut self.pred;
			self.slice
				.iter()
				.enumerate()
				.position(|(idx, bit)| (pred)(idx, bit))
		};
		match idx_opt
		{
			None => self.finish(),
			Some(idx) => unsafe {
				let slice = mem::take(&mut self.slice);
				let (out, rest) = slice.split_at_unchecked_mut_noalias(idx);
				self.slice = rest.get_unchecked_mut(1 ..);
				Some(out)
			},
		}
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		if self.done {
			return None;
		}
		let idx_opt = {
			let pred = &mut self.pred;
			self.slice
				.iter()
				.enumerate()
				.rposition(|(idx, bit)| (pred)(idx, bit))
		};
		match idx_opt
		{
			None => self.finish(),
			Some(idx) => unsafe {
				let slice = mem::take(&mut self.slice);
				let (rest, out) = slice.split_at_unchecked_mut_noalias(idx);
				self.slice = rest;
				Some(out.get_unchecked_mut(1 ..))
			},
		}
	}
});

/** An iterator over subslices separated by bits that match a predicate
function, starting from the end of the slice.

This struct is created by the [`rsplit`] method on [bit slices].

# Original

[`slice::RSplit`](https://doc.rust-lang.org/core/slice/struct.RSplit.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`rsplit`]: struct.BitSlice.html#method.rsplit
**/
#[derive(Clone)]
pub struct RSplit<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	slice: &'a BitSlice<O, T>,
	/// The function used to test whether a split should occur.
	pred: P,
	/// Whether the split is finished.
	done: bool,
}

split!(RSplit => &'a BitSlice<O, T> {
	fn next(&mut self) -> Option<Self::Item> {
		let mut split = Split::<'a, O, T, &mut P> {
			slice: mem::take(&mut self.slice),
			pred: &mut self.pred,
			done: self.done,
		};
		let out = split.next_back();
		self.slice = mem::take(&mut split.slice);
		self.done = split.done;
		out
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let mut split = Split::<'a, O, T, &mut P> {
			slice: mem::take(&mut self.slice),
			pred: &mut self.pred,
			done: self.done,
		};
		let out = split.next();
		self.slice = mem::take(&mut split.slice);
		self.done = split.done;
		out
	}
});

/** An iterator over subslices separated by bits that match a predicate
function, starting from the end of the slice.

This struct is created by the [`rsplit_mut`] method on [bit slices].

# Original

[`slice::RSplit`](https://doc.rust-lang.org/core/slice/struct.RSplit.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`rsplit_mut`]: struct.BitSlice.html#method.rsplit_mut
**/
pub struct RSplitMut<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	slice: &'a mut BitSlice<O, T::Alias>,
	pred: P,
	done: bool,
}

split!(RSplitMut => &'a mut BitSlice<O, T::Alias> {
	fn next(&mut self) -> Option<Self::Item> {
		let mut split = SplitMut::<'a, O, T, &mut P> {
			slice: mem::take(&mut self.slice),
			pred: &mut self.pred,
			done: self.done,
		};
		let out = split.next_back();
		self.slice = mem::take(&mut split.slice);
		self.done = split.done;
		out
	}

	fn next_back(&mut self) -> Option<Self::Item> {
		let mut split = SplitMut::<'a, O, T, &mut P> {
			slice: mem::take(&mut self.slice),
			pred: &mut self.pred,
			done: self.done,
		};
		let out = split.next();
		self.slice = mem::take(&mut split.slice);
		self.done = split.done;
		out
	}
});

/// An internal abstraction over the splitting iterators, so that `splitn`,
/// `splitn_mut`, etc, can be implemented once.
#[doc(hidden)]
trait SplitIter: DoubleEndedIterator {
	/// Marks the underlying iterator as complete, extracting the remaining
	/// portion of the slice.
	fn finish(&mut self) -> Option<Self::Item>;
}

/** An iterator over subslices separated by bits that match a predicate
function, limited to a given number of splits.

This struct is created by the [`splitn`] method on [bit slices].

# Original

[`slice::SplitN`](https://doc.rust-lang.org/core/slice/struct.SplitN.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`splitn`]: struct.BitSlice.html#method.splitn
**/
pub struct SplitN<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	inner: Split<'a, O, T, P>,
	/// The number of splits remaining.
	count: usize,
}

/** An iterator over subslices separated by bits that match a predicate
function, limited to a given number of splits.

This struct is created by the [`splitn_mut`] method on [bit slices].

# Original

[`slice::SplitNMut`](https://doc.rust-lang.org/core/slice/struct.SplitNMut.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`splitn_mut`]: struct.BitSlice.html#method.splitn_mut
**/
pub struct SplitNMut<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	inner: SplitMut<'a, O, T, P>,
	/// The number of splits remaining.
	count: usize,
}

/** An iterator over subslices separated by bits that match a predicate
function, limited to a given number of splits, starting from the end of the
slice.

This struct is created by the [`rsplitn`] method on [bit slices].

# Original

[`slice::RSplitN`](https://doc.rust-lang.org/core/slice/struct.RSplitN.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`rsplitn`]: struct.BitSlice.html#method.rsplitn
**/
pub struct RSplitN<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	inner: RSplit<'a, O, T, P>,
	/// The number of splits remaining.
	count: usize,
}

/** An iterator over subslices separated by bits that match a predicate
function, limited to a given number of splits, starting from the end of the
slice.

This struct is created by the [`rsplitn_mut`] method on [bit slices].

# Original

[`slice::RSplitNMut`](https://doc.rust-lang.org/core/slice/struct.RSplitNMut.html)

# API Differences

In order to allow more than one bit of information for the split decision, the
predicate receives the index of each bit, as well as its value.

[bit slices]: struct.BitSlice.html
[`rsplitn_mut`]: struct.BitSlice.html#method.rsplitn_mut
**/
pub struct RSplitNMut<'a, O, T, P>
where
	O: BitOrder,
	T: 'a + BitStore,
	P: FnMut(usize, &bool) -> bool,
{
	/// The `BitSlice` being split.
	inner: RSplitMut<'a, O, T, P>,
	/// The number of splits remaining.
	count: usize,
}

macro_rules! split_n {
	($outer:ident => $inner:ident => $item:ty $( where $alias:ident )?) => {
		impl<'a, O, T, P> $outer<'a, O, T, P>
		where
			O: BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
		{
			pub(super) fn new(
				slice: $item,
				pred: P,
				count: usize,
			) -> Self
			{Self{
				inner: <$inner<'a, O, T, P>>::new(slice, pred),
				count,
			}}
		}

		impl<O, T, P> Debug for $outer<'_, O, T, P>
		where
			O: BitOrder,
			T: BitStore,
			P: FnMut(usize, &bool) -> bool
		{
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				fmt.debug_struct(stringify!($outer))
					.field("slice", &self.inner.slice)
					.field("count", &self.count)
					.finish()
			}
		}

		impl<'a, O, T, P> Iterator for $outer<'a, O, T, P>
		where
			O: 'a + BitOrder,
			T: 'a + BitStore,
			P: FnMut(usize, &bool) -> bool,
			$( T::$alias: radium::Radium<<<T as BitStore>::Alias as BitStore>::Mem>, )?
		{
			type Item = <$inner <'a, O, T, P> as Iterator>::Item;

			#[inline]
			fn next(&mut self) -> Option<Self::Item> {
				match self.count {
					0 => None,
					1 => {
						self.count -= 1;
						self.inner.finish()
					},
					_ => {
						self.count -= 1;
						self.inner.next()
					},
				}
			}

			#[inline]
			fn size_hint(&self) -> (usize, Option<usize>) {
				let (low, hi) = self.inner.size_hint();
				(low, hi.map(|h| cmp::min(self.count, h)))
			}
		}

		impl<O, T, P> core::iter::FusedIterator for $outer<'_, O, T, P>
		where
			O: BitOrder,
			T: BitStore,
			P: FnMut(usize, &bool) -> bool,
			$( T::$alias: radium::Radium<<<T as BitStore>::Alias as BitStore>::Mem>, )?
		{
		}
	};
}

split_n!(SplitN => Split => &'a BitSlice<O, T>);
split_n!(SplitNMut => SplitMut => &'a mut BitSlice<O, T::Alias> );
split_n!(RSplitN => RSplit => &'a BitSlice<O, T>);
split_n!(RSplitNMut => RSplitMut => &'a mut BitSlice<O, T::Alias> );

#[cfg(test)]
mod tests {
	use crate::prelude::*;

	#[test]
	fn iter() {
		let data = 0b0110_1001u8;
		let bits = data.view_bits::<Msb0>();
		let mut iter = bits.iter();

		assert_eq!(iter.as_bitslice(), bits);
		assert_eq!(iter.next(), Some(&false));
		assert_eq!(iter.as_bitslice(), &bits[1 ..]);
		assert_eq!(iter.next(), Some(&true));

		assert_eq!(iter.as_bitslice(), &bits[2 ..]);
		assert_eq!(iter.next_back(), Some(&true));
		assert_eq!(iter.as_bitslice(), &bits[2 .. 7]);
		assert_eq!(iter.next_back(), Some(&false));

		assert_eq!(iter.as_bitslice(), &bits[2 .. 6]);
		assert_eq!(iter.next(), Some(&true));
		assert_eq!(iter.as_bitslice(), &bits[3 .. 6]);
		assert_eq!(iter.next(), Some(&false));

		assert_eq!(iter.as_bitslice(), &bits[4 .. 6]);
		assert_eq!(iter.next_back(), Some(&false));
		assert_eq!(iter.as_bitslice(), &bits[4 .. 5]);

		assert_eq!(iter.next_back(), Some(&true));
		assert!(iter.as_bitslice().is_empty());
		assert!(iter.next().is_none());
		assert!(iter.next_back().is_none());
	}

	#[test]
	fn iter_mut() {
		let mut data = 0b0110_1001u8;
		let bits = data.view_bits_mut::<Msb0>();
		let mut iter = bits.iter_mut();

		*iter.next().unwrap() = true;
		*iter.nth_back(1).unwrap() = true;
		*iter.nth(2).unwrap() = true;
		*iter.next_back().unwrap() = true;

		assert_eq!(iter.into_bitslice().bitptr(), bits[4 .. 5].bitptr());
	}

	#[test]
	fn windows() {
		let data = 0u8;
		let bits = data.view_bits::<LocalBits>();

		let mut windows = bits.windows(5);
		assert_eq!(windows.next().unwrap().bitptr(), bits[.. 5].bitptr());
		assert_eq!(windows.next_back().unwrap().bitptr(), bits[3 ..].bitptr());

		let mut windows = bits.windows(3);
		assert_eq!(windows.nth(2).unwrap().bitptr(), bits[2 .. 5].bitptr());
		assert_eq!(windows.nth_back(2).unwrap().bitptr(), bits[3 .. 6].bitptr());
		assert!(windows.next().is_none());
		assert!(windows.next_back().is_none());
		assert!(windows.nth(1).is_none());
		assert!(windows.nth_back(1).is_none());
	}

	#[test]
	fn chunks() {
		let data = 0u16;
		let bits = data.view_bits::<LocalBits>();

		let mut chunks = bits.chunks(5);
		assert_eq!(chunks.next().unwrap().bitptr(), bits[.. 5].bitptr());
		assert_eq!(chunks.next_back().unwrap().bitptr(), bits[15 ..].bitptr());

		let mut chunks = bits.chunks(3);
		assert_eq!(chunks.nth(2).unwrap().bitptr(), bits[6 .. 9].bitptr());
		assert_eq!(chunks.nth_back(2).unwrap().bitptr(), bits[9 .. 12].bitptr());
	}

	#[test]
	fn chunks_mut() {
		let mut data = 0u16;
		let bits = data.view_bits_mut::<LocalBits>();
		let (one, two, three, four) = (
			bits[.. 5].bitptr(),
			bits[15 ..].bitptr(),
			bits[6 .. 9].bitptr(),
			bits[9 .. 12].bitptr(),
		);

		let mut chunks = bits.chunks_mut(5);
		assert_eq!(chunks.next().unwrap().bitptr(), one);
		assert_eq!(chunks.next_back().unwrap().bitptr(), two);

		let mut chunks = bits.chunks_mut(3);
		assert_eq!(chunks.nth(2).unwrap().bitptr(), three);
		assert_eq!(chunks.nth_back(2).unwrap().bitptr(), four);
	}

	#[test]
	fn chunks_exact() {
		let data = 0u32;
		let bits = data.view_bits::<LocalBits>();

		let mut chunks = bits.chunks_exact(5);
		assert_eq!(chunks.remainder().bitptr(), bits[30 ..].bitptr());
		assert_eq!(chunks.next().unwrap().bitptr(), bits[.. 5].bitptr());
		assert_eq!(
			chunks.next_back().unwrap().bitptr(),
			bits[25 .. 30].bitptr(),
		);
		assert_eq!(chunks.nth(1).unwrap().bitptr(), bits[10 .. 15].bitptr());
		assert_eq!(
			chunks.nth_back(1).unwrap().bitptr(),
			bits[15 .. 20].bitptr(),
		);

		assert!(chunks.next().is_none());
		assert!(chunks.next_back().is_none());
		assert!(chunks.nth(1).is_none());
		assert!(chunks.nth_back(1).is_none());
	}

	#[test]
	fn chunks_exact_mut() {
		let mut data = 0u32;
		let bits = data.view_bits_mut::<LocalBits>();

		let (one, two, three, four, rest) = (
			bits[.. 5].bitptr(),
			bits[10 .. 15].bitptr(),
			bits[15 .. 20].bitptr(),
			bits[25 .. 30].bitptr(),
			bits[30 ..].bitptr(),
		);

		let mut chunks = bits.chunks_exact_mut(5);
		assert_eq!(chunks.next().unwrap().bitptr(), one);
		assert_eq!(chunks.next_back().unwrap().bitptr(), four);
		assert_eq!(chunks.nth(1).unwrap().bitptr(), two);
		assert_eq!(chunks.nth_back(1).unwrap().bitptr(), three);

		assert!(chunks.next().is_none());
		assert!(chunks.next_back().is_none());
		assert!(chunks.nth(1).is_none());
		assert!(chunks.nth_back(1).is_none());

		assert_eq!(chunks.into_remainder().bitptr(), rest);
	}

	#[test]
	fn rchunks() {
		let data = 0u16;
		let bits = data.view_bits::<LocalBits>();

		let mut rchunks = bits.rchunks(5);
		assert_eq!(rchunks.next().unwrap().bitptr(), bits[11 ..].bitptr());
		assert_eq!(rchunks.next_back().unwrap().bitptr(), bits[.. 1].bitptr());

		let mut rchunks = bits.rchunks(3);
		assert_eq!(rchunks.nth(2).unwrap().bitptr(), bits[7 .. 10].bitptr());
		assert_eq!(rchunks.nth_back(2).unwrap().bitptr(), bits[4 .. 7].bitptr());
	}

	#[test]
	fn rchunks_mut() {
		let mut data = 0u16;
		let bits = data.view_bits_mut::<LocalBits>();
		let (one, two, three, four) = (
			bits[11 ..].bitptr(),
			bits[.. 1].bitptr(),
			bits[7 .. 10].bitptr(),
			bits[4 .. 7].bitptr(),
		);

		let mut rchunks = bits.rchunks_mut(5);
		assert_eq!(rchunks.next().unwrap().bitptr(), one);
		assert_eq!(rchunks.next_back().unwrap().bitptr(), two);

		let mut rchunks = bits.rchunks_mut(3);
		assert_eq!(rchunks.nth(2).unwrap().bitptr(), three);
		assert_eq!(rchunks.nth_back(2).unwrap().bitptr(), four);
	}

	#[test]
	fn rchunks_exact() {
		let data = 0u32;
		let bits = data.view_bits::<LocalBits>();

		let mut rchunks = bits.rchunks_exact(5);
		assert_eq!(rchunks.remainder().bitptr(), bits[.. 2].bitptr());
		assert_eq!(rchunks.next().unwrap().bitptr(), bits[27 ..].bitptr());
		assert_eq!(rchunks.next_back().unwrap().bitptr(), bits[2 .. 7].bitptr(),);
		assert_eq!(rchunks.nth(1).unwrap().bitptr(), bits[17 .. 22].bitptr());
		assert_eq!(
			rchunks.nth_back(1).unwrap().bitptr(),
			bits[12 .. 17].bitptr(),
		);

		assert!(rchunks.next().is_none());
		assert!(rchunks.next_back().is_none());
		assert!(rchunks.nth(1).is_none());
		assert!(rchunks.nth_back(1).is_none());
	}

	#[test]
	fn rchunks_exact_mut() {
		let mut data = 0u32;
		let bits = data.view_bits_mut::<LocalBits>();

		let (rest, one, two, three, four) = (
			bits[.. 2].bitptr(),
			bits[2 .. 7].bitptr(),
			bits[12 .. 17].bitptr(),
			bits[17 .. 22].bitptr(),
			bits[27 ..].bitptr(),
		);

		let mut rchunks = bits.rchunks_exact_mut(5);
		assert_eq!(rchunks.next().unwrap().bitptr(), four);
		assert_eq!(rchunks.next_back().unwrap().bitptr(), one);
		assert_eq!(rchunks.nth(1).unwrap().bitptr(), three);
		assert_eq!(rchunks.nth_back(1).unwrap().bitptr(), two);

		assert!(rchunks.next().is_none());
		assert!(rchunks.next_back().is_none());
		assert!(rchunks.nth(1).is_none());
		assert!(rchunks.nth_back(1).is_none());

		assert_eq!(rchunks.into_remainder().bitptr(), rest);
	}
}
