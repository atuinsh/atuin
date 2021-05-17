//! Port of the `Vec<T>` function API.

use crate::{
	mem::BitMemory,
	order::BitOrder,
	pointer::BitPtr,
	slice::BitSlice,
	store::BitStore,
	vec::{
		iter::{
			Drain,
			Splice,
		},
		BitVec,
	},
};

use alloc::{
	borrow::ToOwned,
	boxed::Box,
	vec::Vec,
};

use core::{
	mem,
	ops::RangeBounds,
	slice,
};

use funty::IsInteger;

use tap::{
	pipe::Pipe,
	tap::Tap,
};

impl<O, T> BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Constructs a new, empty `BitVec<O, T>`.
	///
	/// The vector will not allocate until bits are pushed into it.
	///
	/// # Original
	///
	/// [`Vec::new`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.new)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = BitVec::<LocalBits, usize>::new();
	/// ```
	#[inline]
	pub fn new() -> Self {
		Self {
			pointer: BitPtr::<T>::EMPTY.to_nonnull(),
			capacity: 0,
		}
	}

	/// Constructs a new, empty `BitVec<O, T>` with the specified capacity.
	///
	/// The vector will be able to hold at least `capacity` bits without
	/// reällocating. If `capacity` is 0, the vector will not allocate.
	///
	/// It is important to note that although the returned vector has the
	/// *capacity* specified, the vector will have a zero *length*. For an
	/// explanation of the difference between length and capacity, see
	/// *[Capacity and reällocation]*.
	///
	/// # Original
	///
	/// [`Vec::with_capacity`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.with_capacity)
	///
	/// # Panics
	///
	/// Panics if the requested capacity exceeds the vector’s limits.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = BitVec::<LocalBits, usize>::with_capacity(10);
	///
	/// // The vector contains no items, even though it has capacity for more
	/// assert_eq!(bv.len(), 0);
	///
	/// // These are all done without reallocating...
	/// for i in 0..10 {
	///   bv.push(true);
	/// }
	///
	/// // ...but this may make the vector reallocate
	/// bv.push(false);
	/// ```
	///
	/// [Capacity and reällocation]: #capacity-and-reallocation
	#[inline]
	pub fn with_capacity(capacity: usize) -> Self {
		assert!(
			capacity <= BitSlice::<O, T>::MAX_BITS,
			"Vector capacity exceeded: {} > {}",
			capacity,
			BitSlice::<O, T>::MAX_BITS
		);
		let vec = capacity
			.pipe(crate::mem::elts::<T>)
			.pipe(Vec::<T>::with_capacity);
		let (ptr, capacity) = (vec.as_ptr(), vec.capacity());
		mem::forget(vec);
		ptr.pipe(BitPtr::uninhabited)
			.pipe(BitPtr::to_nonnull)
			.pipe(|pointer| Self { pointer, capacity })
	}

	/// Creates a `BitVec<O, T>` directly from the raw components of another
	/// bit-vector.
	///
	/// # Original
	///
	/// [`Vec::from_raw_parts`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.from_raw_parts)
	///
	/// # API Differences
	///
	/// Ordinary vectors decompose into their buffer pointer and element length
	/// separately; bit vectors must keep these two components bundled into the
	/// `*BitSlice` region pointer. As such, this only accepts two components;
	/// the slice pointer and the buffer capacity.
	///
	/// `Vec` could define its raw parts as `*[T]` and `usize` also, but Rust
	/// does not make working with raw slice pointers easy.
	///
	/// # Panics
	///
	/// This function panics if `pointer` is the null pointer.
	///
	/// # Safety
	///
	/// This is highly unsafe, due to the number of invariants that aren’t
	/// checked:
	///
	/// - `pointer` needs to have been previously allocated via `BitVec<O, T>`
	///   (at least, it’s highly likely to be incorrect if it wasn’t).
	/// - `T` needs to have the same size and alignment as what `pointer` was
	///   allocated with. (`T` having a less strict alignment is not sufficient;
	///   the alignment really needs to be equal to satisfy the [`dealloc`]
	///   requirement that memory must be allocated and deällocated with the
	///   same layout.)
	/// - `capacity` needs to be the capacity that the pointer was allocated
	///   with.
	///
	/// In addition to the invariants inherited from `Vec::from_raw_parts`, the
	/// fact that this function takes a bit-slice pointer adds another one:
	///
	/// - **`pointer` MUST NOT have had its value modified in any way in the**
	/// **time when it was outside of a `bitvec` container type.**
	///
	/// Violating these *will* cause problems like corrupting the allocator’s
	/// internal data structures. For example it is **not** safe to build a
	/// `BitVec<_, u8>` from a pointer to a C `char` array with length `size_t`.
	/// It’s also not safe to build one from a `BitVec<_, u16>` and its length,
	/// becauset the allocator cares about the alignment, and these two types
	/// have different alignments. The buffer was allocated with alignment 2
	/// (for `u16`), but after turning it into a `BitVec<_, u8>`, it’ll be
	/// deällocated with alignment 1.
	///
	/// The ownership of `pointer` is effectively transferred to the `BitVec<O,
	/// T>` which may then deällocate, reällocate, or change the contents of
	/// memory pointed to by the pointer at will. Ensure that nothing else uses
	/// the pointer after calling this function.
	///
	/// # Examples
	///
	/// ```rust
	/// # extern crate core;
	/// use bitvec::prelude::*;
	/// use bitvec as bv;
	/// use core::mem;
	///
	/// let bv = bitvec![0, 1, 0, 1];
	///
	/// // Prevent running `bv`’s destructor so we are in complete control
	/// // of the allocation.
	/// let mut bv = mem::ManuallyDrop::new(bv);
	///
	/// // Pull out the various important pieces of information about `bv`
	/// let p = bv.as_mut_ptr();
	/// let e = bv.elements();
	/// let cap = bv.capacity();
	///
	/// unsafe {
	///   let bits = bv::slice::from_raw_parts_mut::<LocalBits, _>(p, e);
	///   let len = bits.len();
	///
	///   // Overwrite memory with a new pattern
	///   bits.iter_mut().for_each(|mut b| *b = true);
	///
	///   // Put everything back together into a BitVec
	///   let rebuilt = BitVec::from_raw_parts(bits as *mut _, cap);
	///   assert_eq!(rebuilt.len(), len);
	/// }
	/// ```
	#[inline]
	pub unsafe fn from_raw_parts(
		pointer: *mut BitSlice<O, T>,
		capacity: usize,
	) -> Self
	{
		if (pointer as *mut [()]).is_null() {
			panic!("Attempted to reconstruct a `BitVec` from a null pointer");
		}
		pointer
			.pipe(BitPtr::from_bitslice_ptr_mut)
			.to_nonnull()
			.pipe(|pointer| Self { pointer, capacity })
	}

	/// Returns the number of bits the vector can hold without reällocating.
	///
	/// # Original
	///
	/// [`Vec::capacity`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.capacity)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv: BitVec<LocalBits, usize> = BitVec::with_capacity(100);
	/// assert!(bv.capacity() >= 100);
	/// ```
	#[inline]
	pub fn capacity(&self) -> usize {
		self.capacity
			.checked_mul(T::Mem::BITS as usize)
			.expect("Vector capacity exceeded")
			//  Don’t forget to subtract any dead bits in the front of the base!
			//  This has to be saturating, becase a non-zero head on a zero
			//  capacity underflows.
			.saturating_sub(self.bitptr().head().value() as usize)
	}

	/// Reserves capacity for at least `additional` more bits to be inserted in
	/// the given `BitVec<O, T>`. The collection may reserve more space to avoid
	/// frequent reällocations. After calling `reserve`, capacity will be
	/// greater than or equal to `self.len() + additional`. Does nothing if
	/// capacity is already sufficient.
	///
	/// # Original
	///
	/// [`Vec::reserve`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.reserve)
	///
	/// # Panics
	///
	/// Panics if the new capacity exceeds the vector’s limits.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1];
	/// bv.reserve(100);
	/// assert!(bv.capacity() >= 101);
	/// ```
	#[inline]
	pub fn reserve(&mut self, additional: usize) {
		let len = self.len();
		let new_len = len
			.checked_add(additional)
			.expect("Vector capacity exceeded");
		assert!(
			new_len <= BitSlice::<O, T>::MAX_BITS,
			"Vector capacity exceeded: {} > {}",
			new_len,
			BitSlice::<O, T>::MAX_BITS
		);
		let bitptr = self.bitptr();
		let head = bitptr.head();
		let elts = bitptr.elements();
		//  Only reserve if the request needs new elements.
		if let Some(extra) = head.span(new_len).0.checked_sub(elts) {
			self.with_vec(|v| v.reserve(extra));
			let capa = self.capacity();
			//  Zero the newly-reserved buffer.
			unsafe { self.get_unchecked_mut(len .. capa) }.set_all(false);
		}
	}

	/// Reserves the minimum capacity for exactly `additional` more bits to be
	/// inserted in the given `BitVec<O, T>`. After calling `reserve_exact`,
	/// capacity will be greater than or equal to `self.len() + additional`.
	/// Does nothing if the capacity is already sufficient.
	///
	/// Note that the allocator may give the collection more space than it
	/// requests. Therefore, capacity can not be relied upon to be precisely
	/// minimal. Prefer `reserve` if future insertions are expected.
	///
	/// # Original
	///
	/// [`Vec::reserve_exact`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.reserve_exact)
	///
	/// # Panics
	///
	/// Panics if the new capacity exceeds the vector’s limits.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1];
	/// bv.reserve_exact(100);
	/// assert!(bv.capacity() >= 101);
	/// ```
	#[inline]
	pub fn reserve_exact(&mut self, additional: usize) {
		let new_len = self
			.len()
			.checked_add(additional)
			.expect("Vector capacity exceeded");
		assert!(
			new_len <= BitSlice::<O, T>::MAX_BITS,
			"Vector capacity exceeded: {} > {}",
			new_len,
			BitSlice::<O, T>::MAX_BITS
		);
		let bitptr = self.bitptr();
		let head = bitptr.head();
		let elts = bitptr.elements();
		//  Only reserve if the request needs new elements.
		if let Some(extra) = head.span(new_len).0.checked_sub(elts) {
			self.with_vec(|v| v.reserve_exact(extra));
		}
	}

	/// Shrinks the capacity of the vector as much as possible.
	///
	/// It will drop down as close as possible to the length but the allocator
	/// may still inform the vector that there is space for a few more bits.
	///
	/// # Original
	///
	/// [`Vec::shrink_to_fit`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.shrink_to_fit)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = BitVec::<LocalBits, usize>::with_capacity(100);
	/// bv.extend([false, true, false].iter().copied());
	/// assert!(bv.capacity() >= 100);
	/// bv.shrink_to_fit();
	/// assert!(bv.capacity() >= 3);
	/// ```
	#[inline]
	pub fn shrink_to_fit(&mut self) {
		self.with_vec(|v| v.shrink_to_fit());
	}

	/// Converts the vector into [`Box<[T]>`].
	///
	/// Note that this will drop any excess capacity.
	///
	/// # Original
	///
	/// [`Vec::into_boxed_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.into_boxed_slice)
	///
	/// # Analogue
	///
	/// See [`into_boxed_bitslice`] for a `BitVec -> BitBox` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![0, 1, 0];
	///
	/// let slice = bv.into_boxed_slice();
	/// assert_eq!(slice.len(), 1);
	/// ```
	///
	/// Any excess capacity is removed:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv: BitVec = BitVec::with_capacity(100);
	/// bv.extend([false, true, false].iter().copied());
	///
	/// assert!(bv.capacity() >= 100);
	/// let slice = bv.into_boxed_slice();
	/// assert_eq!(slice.into_vec().capacity(), 1);
	/// ```
	///
	/// [`Box<[T]>`]: https://doc.rust-lang.org/alloc/boxed/struct.Box.html
	/// [`into_boxed_bitslice`]: #method.into_boxed_bitslice
	#[inline]
	pub fn into_boxed_slice(self) -> Box<[T]> {
		self.into_vec().into_boxed_slice()
	}

	/// Shortens the vector, keeping the first `len` bits and dropping the rest.
	///
	/// If `len` is greater than the vector’s current length, this has no
	/// effect.
	///
	/// The [`drain`] method can emulate `truncate`, but causes the excess bits
	/// to be returned instead of dropped.
	///
	/// Note that this method has no effect on the allocated capacity of the
	/// vector, **nor does it erase truncated memory**. Bits in the allocated
	/// memory that are outside of the `.as_bitslice()` view always have
	/// **unspecified** values, and cannot be relied upon to be zero.
	///
	/// # Original
	///
	/// [`Vec::truncate`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.truncate)
	///
	/// # Examples
	///
	/// Truncating a five bit vector to two bits:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1; 5];
	/// bv.truncate(2);
	/// assert_eq!(bv.len(), 2);
	/// assert!(bv.as_slice()[0].count_ones() >= 5);
	/// ```
	///
	/// No truncation occurs when `len` is greater than the vector’s current
	/// length:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1; 3];
	/// bv.truncate(8);
	/// assert_eq!(bv.len(), 3);
	/// ```
	///
	/// Truncating when `len == 0` is equivalent to calling the [`clear`]
	/// method.
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0; 3];
	/// bv.truncate(0);
	/// assert!(bv.is_empty());
	/// ```
	///
	/// [`clear`]: #method.clear
	/// [`drain`]: #method.drain
	#[inline]
	pub fn truncate(&mut self, len: usize) {
		if len < self.len() {
			unsafe { self.set_len(len) }
		}
	}

	/// Extracts an element slice containing the entire vector.
	///
	/// # Original
	///
	/// [`Vec::as_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_slice)
	///
	/// # Analogue
	///
	/// See [`as_bitslice`] for a `&BitVec -> &BitSlice` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	/// use std::io::{self, Write};
	/// let buffer = bitvec![Msb0, u8; 0, 1, 0, 1, 1, 0, 0, 0];
	/// io::sink().write(buffer.as_slice()).unwrap();
	/// # }
	/// ```
	///
	/// [`as_bitslice`]: #method.as_bitslice
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_slice(&self) -> &[T] {
		let bitptr = self.bitptr();
		let (base, elts) = (bitptr.pointer().to_const(), bitptr.elements());
		unsafe { slice::from_raw_parts(base, elts) }
	}

	/// Extracts a mutable slice of the entire vector.
	///
	/// # Original
	///
	/// [`Vec::as_mut_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_mut_slice)
	///
	/// # Analogue
	///
	/// See [`as_mut_bitslice`] for a `&mut BitVec -> &mut BitSlice` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	/// use std::io::{self, Read};
	/// let mut buffer = bitvec![Msb0, u8; 0; 24];
	/// io::repeat(0b101).read_exact(buffer.as_mut_slice()).unwrap();
	/// # }
	/// ```
	///
	/// [`as_mut_bitslice`]: #method.as_mut_bitslice
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_slice(&mut self) -> &mut [T] {
		let bitptr = self.bitptr();
		let (base, elts) = (bitptr.pointer().to_mut(), bitptr.elements());
		unsafe { slice::from_raw_parts_mut(base, elts) }
	}

	/// Returns a raw pointer to the vector’s buffer.
	///
	/// The caller must ensure that the vector outlives the pointer this
	/// function returns, or else it will end up pointing to garbage. Modifying
	/// the vector may cause its buffer to be reällocated, which would also make
	/// any pointers to it invalid.
	///
	/// The caller must also ensure that the memory the pointer
	/// (non-transitively) points to is never written to (except inside an
	/// `UnsafeCell`) using this pointer or any pointer derived from it. If you
	/// need to mutate the contents of the slice, use [`as_mut_ptr`].
	///
	/// # Original
	///
	/// [`Vec::as_ptr`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_ptr)
	///
	/// # Analogue
	///
	/// See [`as_bitptr`] for a `&BitVec -> *const BitSlice` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![Lsb0; 0, 1, 0, 1];
	/// let bv_ptr = bv.as_ptr();
	///
	/// unsafe {
	///   assert_eq!(*bv_ptr, 0b1010);
	/// }
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_ptr(&self) -> *const T {
		self.bitptr().pointer().to_const()
	}

	/// Returns an unsafe mutable pointer to the vector’s buffer.
	///
	/// The caller must ensure that the vector outlives the pointer this
	/// function returns, or else it will end up pointing to garbage. Modifying
	/// the vector may cause its buffer to be reällocated, which would also make
	/// any pointers to it invalid.
	///
	/// # Original
	///
	/// [`Vec::as_mut_ptr`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_mut_ptr)
	///
	/// # Analogue
	///
	/// See [`as_mut_bitptr`] for a `&mut BitVec -> *mut BitSlice` transform.
	///
	/// # Eaxmples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let size = 4;
	/// let mut bv: BitVec<Msb0, usize> = BitVec::with_capacity(size);
	/// let bv_ptr = bv.as_mut_ptr();
	///
	/// unsafe {
	///   *bv_ptr = !0;
	///   bv.set_len(size);
	/// }
	/// assert_eq!(bv.len(), 4);
	/// assert!(bv.all());
	/// ```
	///
	/// [`as_mut_bitptr`]: #method.as_mut_bitptr
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_ptr(&mut self) -> *mut T {
		self.bitptr().pointer().to_mut()
	}

	/// Forces the length of the vector to `new_len`.
	///
	/// This is a low-level operation that maintains none of the normal
	/// invariants of the type. Normally changing the length of a vector is done
	/// using one of the safe operations instead, such as [`truncate`],
	/// [`resize`], [`extend`], or [`clear`].
	///
	/// # Original
	///
	/// [`Vec::set_len`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.set_len)
	///
	/// # Safety
	///
	/// - `new_len` must be less than or equal to [`capacity()`].
	///
	/// # Examples
	///
	/// This method can be useful for situations in which the vector is serving
	/// as a buffer for other code, particularly over FFI:
	///
	/// ```rust
	/// # #![allow(dead_code)]
	/// # #![allow(improper_ctypes)]
	/// # const ERL_OK: i32 = 0;
	/// # extern "C" {
	/// #   fn erl_read_bits(
	/// #     bv: *mut BitVec<Msb0, u8>,
	/// #     bits_reqd: usize,
	/// #     bits_read: *mut usize,
	/// #   ) -> i32;
	/// # }
	/// use bitvec::prelude::*;
	///
	/// // `bitvec` could pair with `rustler` for a better bitstream
	/// type ErlBitstring = BitVec<Msb0, u8>;
	/// # pub fn _test() {
	/// let mut bits_read = 0;
	/// // An imaginary Erlang function wants a large bit buffer.
	/// let mut buf = ErlBitstring::with_capacity(32_768);
	/// // SAFETY: When `erl_read_bits` returns `ERL_OK`, it holds that:
	/// // 1. `bits_read` bits were initialized.
	/// // 2. `bits_read` <= the capacity (32_768)
	/// // which makes `set_len` safe to call.
	/// unsafe {
	///   // Make the FFI call...
	///   let status = erl_read_bits(&mut buf, 10, &mut bits_read);
	///   if status == ERL_OK {
	///     // ...and update the length to what was read in.
	///     buf.set_len(bits_read);
	///   }
	/// }
	/// # }
	/// ```
	///
	/// [`capacity()`]: #method.capacity
	/// [`clear`]: #method.clear
	/// [`extend`]: #method.extend
	/// [`resize`]: #method.resize
	/// [`truncate`]: #method.truncate
	#[inline]
	pub unsafe fn set_len(&mut self, new_len: usize) {
		assert!(
			new_len <= BitPtr::<T>::REGION_MAX_BITS,
			"Capacity exceeded: {} exceeds maximum length {}",
			new_len,
			BitPtr::<T>::REGION_MAX_BITS,
		);
		let cap = self.capacity();
		assert!(
			new_len <= cap,
			"Capacity exceeded: {} exceeds allocation size {}",
			new_len,
			cap,
		);
		self.pointer = self
			.pointer
			.as_ptr()
			.pipe(BitPtr::from_bitslice_ptr_mut)
			.tap_mut(|bp| bp.set_len(new_len))
			.to_nonnull()
	}

	/// Removes a bit from the vector and returns it.
	///
	/// The removed bit is replaced by the last bit of the vector.
	///
	/// This does not preserve ordering, but is O(1).
	///
	/// # Original
	///
	/// [`Vec::swap_remove`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.swap_remove)
	///
	/// # Panics
	///
	/// Panics if `index` is out of bounds.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 0, 1, 0, 1];
	/// assert!(!bv.swap_remove(1));
	/// assert_eq!(bv, bits![0, 1, 1, 0]);
	///
	/// assert!(!bv.swap_remove(0));
	/// assert_eq!(bv, bits![0, 1, 1]);
	/// ```
	#[inline]
	pub fn swap_remove(&mut self, index: usize) -> bool {
		let len = self.len();
		assert!(index < len, "Index {} out of bounds: {}", index, len);
		let last = len - 1;
		//  TODO(myrrlyn): Implement `BitSlice::xchg`?
		unsafe {
			self.swap_unchecked(index, last);
			self.set_len(last);
			*self.get_unchecked(last)
		}
	}

	/// Inserts a bit at position `index` within the vector, shifting all bits
	/// after it to the right.
	///
	/// # Original
	///
	/// [`Vec::insert`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.insert)
	///
	/// # Panics
	///
	/// Panics if `index > len`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0; 5];
	/// bv.insert(4, true);
	/// assert_eq!(bv, bits![0, 0, 0, 0, 1, 0]);
	/// bv.insert(2, true);
	/// assert_eq!(bv, bits![0, 0, 1, 0, 0, 1, 0]);
	/// ```
	#[inline]
	pub fn insert(&mut self, index: usize, value: bool) {
		let len = self.len();
		assert!(index <= len, "Index {} out of bounds: {}", index, len);
		self.push(value);
		unsafe { self.get_unchecked_mut(index ..) }.rotate_right(1);
	}

	/// Removes and returns the bit at position `index` within the vector,
	/// shifting all bits after it to the left.
	///
	/// # Original
	///
	/// [`Vec::remove`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.remove)
	///
	/// # Panics
	///
	/// Panics if `index` is out of bounds.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1, 0];
	/// assert!(bv.remove(1));
	/// assert_eq!(bv, bits![0, 0]);
	/// ```
	#[inline]
	pub fn remove(&mut self, index: usize) -> bool {
		let len = self.len();
		assert!(index < len, "Index {} out of bounds: {}", index, len);
		let last = len - 1;
		unsafe {
			self.get_unchecked_mut(index ..).rotate_left(1);
			self.set_len(last);
			*self.get_unchecked(last)
		}
	}

	/// Retains only the bits specified by the predicate.
	///
	/// In other words, remove all bits `b` such that `func(idx(b), &b)` returns
	/// `false`. This method operates in place, visiting each bit exactly once
	/// in the original order, and preserves the order of the retained bits.
	///
	/// # Original
	///
	/// [`Vec::retain`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.retain)
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
	/// let mut bv = bitvec![0, 1, 1, 0, 0, 1];
	/// bv.retain(|i, b| (i % 2 == 0) ^ b);
	/// assert_eq!(bv, bits![0, 1, 0, 1]);
	/// ```
	#[inline]
	pub fn retain<F>(&mut self, mut func: F)
	where F: FnMut(usize, &bool) -> bool {
		for n in (0 .. self.len()).rev() {
			if !func(n, unsafe { self.get_unchecked(n) }) {
				self.remove(n);
			}
		}
	}

	/// Appends a bit to the back of a collection.
	///
	/// # Original
	///
	/// [`Vec::push`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.push)
	///
	/// # Panics
	///
	/// Panics if the number of bits in the vector exceeds the maximum vector
	/// capacity.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 0];
	/// bv.push(true);
	/// assert_eq!(bv.count_ones(), 1);
	/// ```
	#[inline]
	pub fn push(&mut self, value: bool) {
		let len = self.len();
		assert!(
			len <= BitSlice::<O, T>::MAX_BITS,
			"Exceeded capacity: {} >= {}",
			len,
			BitSlice::<O, T>::MAX_BITS,
		);
		if self.is_empty() || self.bitptr().tail().value() == T::Mem::BITS {
			self.with_vec(|v| v.push(T::Mem::ZERO));
		}
		unsafe {
			self.pointer = self
				.pointer
				.as_ptr()
				.pipe(BitPtr::from_bitslice_ptr_mut)
				.tap_mut(|bp| bp.set_len(len + 1))
				.to_nonnull();
			self.set_unchecked(len, value);
		}
	}

	/// Removes the last bit from a vector and returns it, or [`None`] if it is
	/// empty.
	///
	/// # Original
	///
	/// [`Vec::pop`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.pop)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 0, 1];
	/// assert_eq!(bv.pop(), Some(true));
	/// assert!(bv.not_any());
	/// ```
	///
	/// [`None`]: https://doc.rust-lang.org/core/option/enum.Option.html#variant.None
	#[inline]
	pub fn pop(&mut self) -> Option<bool> {
		match self.len() {
			0 => None,
			n => unsafe {
				let m = n - 1;
				(*self.get_unchecked(m)).tap(|_| self.set_len(m)).pipe(Some)
			},
		}
	}

	/// Moves all the bits of `other` into `self`, leaving `other` empty.
	///
	/// # Original
	///
	/// [`Vec::append`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.append)
	///
	/// # Panics
	///
	/// Panics if the number of bits overflows the maximum vector capacity.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv1 = bitvec![0; 10];
	/// let mut bv2 = bitvec![1; 10];
	///
	/// bv1.append(&mut bv2);
	///
	/// assert_eq!(bv1.count_ones(), 10);
	/// assert!(bv2.is_empty());
	/// ```
	#[inline]
	pub fn append<O2, T2>(&mut self, other: &mut BitVec<O2, T2>)
	where
		O2: BitOrder,
		T2: BitStore,
	{
		self.extend(other.iter().copied());
		other.clear();
	}

	/// Creates a draining iterator that removes the specified range in the
	/// vector and yields the removed items.
	///
	/// Note 1: The bit range is removed even if the iterator is only partially
	/// consumed or not consumed at all.
	///
	/// Note 2: It is unspecified how many bits are removed from the vector if
	/// the `Drain` value is leaked.
	///
	/// # Original
	///
	/// [`Vec::drain`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.drain)
	///
	/// # Panics
	///
	/// Panics if the starting point is greater than the end point or if the end
	/// point is greater than the length of the vector.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1, 1];
	/// let bv2: BitVec = bv.drain(1 ..).collect();
	/// assert_eq!(bv, bits![0]);
	/// assert_eq!(bv2, bits![1, 1]);
	///
	/// // A full range clears the vector
	/// bv.drain(..);
	/// assert_eq!(bv, bits![]);
	/// ```
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn drain<R>(&mut self, range: R) -> Drain<O, T>
	where R: RangeBounds<usize> {
		Drain::new(self, range)
	}

	/// Clears the vector, removing all values.
	///
	/// Note that this method has no effect on the allocated capacity of the
	/// vector.
	///
	/// # Original
	///
	/// [`Vec::clear`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.clear)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1, 0, 1];
	///
	/// bv.clear();
	///
	/// assert!(bv.is_empty());
	/// ```
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub fn clear(&mut self) {
		unsafe {
			self.set_len(0);
		}
	}

	/// Splits the collection into two at the given index.
	///
	/// Returns a newly allocated vector containing the elements in range `[at,
	/// len)`. After the call, the original vector will be left containing the
	/// bits `[0, at)` with its previous capacity unchanged.
	///
	/// # Original
	///
	/// [`Vec::split_off`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.split_off)
	///
	/// # Panics
	///
	/// Panics if `at > len`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 0, 1];
	/// let bv2 = bv.split_off(1);
	/// assert_eq!(bv, bits![0]);
	/// assert_eq!(bv2, bits![0, 1]);
	/// ```
	#[inline]
	pub fn split_off(&mut self, at: usize) -> Self {
		let len = self.len();
		assert!(at <= len, "Index {} out of bounds: {}", at, len);
		match at {
			0 => mem::replace(self, Self::with_capacity(self.capacity())),
			n if n == len => Self::new(),
			_ => unsafe {
				self.set_len(at);
				self.get_unchecked(at .. len).to_owned()
			},
		}
	}

	/// Resizes the `BitVec` in-place so that `len` is equal to `new_len`.
	///
	/// If `new_len` is greater than `len`, the `BitVec` is extended by the
	/// difference, with each additional slot filled with the result of calling
	/// the closure `func`. The return values from `func` will end up in the
	/// `BitVec` in the order they have been generated.
	///
	/// If `new_len` is less than `len`, the `Vec` is simply truncated.
	///
	/// This method uses a closure to create new values on every push. If you’d
	/// rather [`Clone`] a given bit, use [`resize`]. If you want to use the
	/// [`Default`] trait to generate values, you can pass [`Default::default`]
	/// as the second argument.
	///
	/// # Original
	///
	/// [`Vec::resize_with`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.resize_with)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1; 3];
	/// bv.resize_with(5, Default::default);
	/// assert_eq!(bv, bits![1, 1, 1, 0, 0]);
	///
	/// let mut bv = bitvec![];
	/// let mut p = 0;
	/// bv.resize_with(4, || { p += 1; p % 2 == 0 });
	/// assert_eq!(bv, bits![0, 1, 0, 1]);
	/// ```
	///
	/// [`Clone`]: https://doc.rust-lang.org/std/clone/trait.Clone.html
	/// [`Default`]: https://doc.rust-lang.org/std/default/trait.Default.html
	/// [`Default::default`]: https://doc.rust-lang.org/std/default/trait.Default.html#tymethod.default
	/// [`resize`]: #method.resize
	#[inline]
	pub fn resize_with<F>(&mut self, new_len: usize, mut func: F)
	where F: FnMut() -> bool {
		let len = self.len();
		if new_len > len {
			let ext = new_len - len;
			self.reserve(ext);
			unsafe {
				self.get_unchecked_mut(len .. new_len)
					.for_each(|_, _| func());
			}
		}
		unsafe {
			self.set_len(new_len);
		}
	}

	/// Resizes the `BitVec` in-place so that `len` is equal to `new_len`.
	///
	/// If `new_len` is greater than `len`, the `BitVec` is extended by the
	/// difference, with each additional slot filled with `value`. If `new_len`
	/// is less than `len`, the `BitVec` is simply truncated.
	///
	/// This method requires a single `bool` value. If you need more
	/// flexibility, use [`resize_with`].
	///
	/// # Original
	///
	/// [`Vec::resize`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.resize)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1];
	/// bv.resize(3, false);
	/// assert_eq!(bv, bits![1, 0, 0]);
	///
	/// let mut bv = bitvec![1; 4];
	/// bv.resize(2, false);
	/// assert_eq!(bv, bits![1; 2]);
	/// ```
	///
	/// [`resize_with`]: #method.resize_with
	#[inline]
	pub fn resize(&mut self, new_len: usize, value: bool) {
		let len = self.len();
		if new_len > len {
			let ext = new_len - len;
			self.reserve(ext);
			/* Initialize all of the newly-allocated memory, not just the bits
			that will become live. This is a requirement for correctness.

			*Strictly speaking*, only `len .. ⌈new_len / bit_width⌉` needs to be
			initialized, but computing the correct boundary is probably not
			sufficiently less effort than just initializing the complete
			allocation to be worth the instructions. If users complain about
			performance on this method, revisit this decision, but if they don’t
			then the naïve solution is fine.
			*/
			let capa = self.capacity();
			unsafe {
				self.get_unchecked_mut(len .. capa).set_all(value);
			}
		}
		unsafe {
			self.set_len(new_len);
		}
	}

	/// Clones and appends all `bool`s in a slice to the `BitVec`.
	///
	/// Iterates over the slice `other`, clones each `bool`, and then appends it
	/// to the `BitVec`. The `other` slice is traversed in-order.
	///
	/// Prefer the [`Extend`] implementation; this method is retained only for
	/// API compatibility, and offers no performance benefit.
	///
	/// # Original
	///
	/// [`Vec::extend_from_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.extend_from_slice)
	///
	/// # Analogue
	///
	/// See [`extend_from_bitslice`] for the method to append a bit-slice of the
	/// same type parameters to a bit-vector.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0];
	/// bv.extend_from_slice(&[true]);
	/// assert_eq!(bv, bits![0, 1]);
	/// ```
	///
	/// [`extend`]: #impl-Extend<%26'a bool>
	/// [`extend_from_bitslice`]: #method.extend_from_bitslice
	#[cfg_attr(not(tarpaulin), inline(always))]
	pub fn extend_from_slice(&mut self, other: &[bool]) {
		self.extend(other)
	}

	/// Creates a splicing iterator that replaces the specified range in the
	/// vector with the given `replace_with` iterator and yields the removed
	/// items. `replace_with` does not need to be the same length as `range`.
	///
	/// The element range is removed even if the iterator is not consumed until
	/// the end.
	///
	/// It is unspecified how many bits are removed from the vector if the
	/// `Splice` value is leaked.
	///
	/// The input iterator `replace_with` is only consumed when the `Splice`
	/// value is dropped.
	///
	/// This is optimal if:
	///
	/// - the tail (bits in the vector after `range`) is empty
	/// - or `replace_with` yields fewer bits than `range`’s length
	/// - or the lower bound of its `size_hint()` is exact
	///
	/// Otherwise, a temporary vector is allocated and the tail is moved twice.
	///
	/// # Original
	///
	/// [`Vec::splice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.splice)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1, 0];
	/// let new = bits![1, 0];
	/// let old: BitVec = bv.splice(.. 2, new.iter().copied()).collect();
	/// assert_eq!(bv, bits![1, 0, 0]);
	/// assert_eq!(old, bits![0, 1]);
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn splice<R, I>(
		&mut self,
		range: R,
		replace_with: I,
	) -> Splice<O, T, I::IntoIter>
	where
		R: RangeBounds<usize>,
		I: IntoIterator<Item = bool>,
	{
		Splice::new(self.drain(range), replace_with)
	}
}
