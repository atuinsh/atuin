/*! A dynamically-allocated buffer containing a `BitSlice<O, T>` region.

You can read the standard library’s [`alloc::vec` module documentation][std]
here.

This module defines the [`BitVec`] buffer, and all of its associated support
code.

`BitVec` is equivalent to `Vec<bool>`, in its operation and in its relationship
to the `BitSlice` type. Most of the interesting work to be done on a
bit-sequence is implemented in `BitSlice`, to which `BitVec` dereferences, and
the vector container itself only exists to maintain ownership, implement dynamic
resizing, and provide some specializations that cannot safely be done on
`BitSlice` alone.

[`BitVec`]: struct.BitVec.html
[std]: https://doc.rust-lang.org/alloc/vec
!*/

#![cfg(feature = "alloc")]

use crate::{
	boxed::BitBox,
	index::BitIdx,
	mem::BitMemory,
	order::{
		BitOrder,
		Lsb0,
	},
	pointer::BitPtr,
	slice::BitSlice,
	store::BitStore,
};

use alloc::vec::Vec;

use core::{
	mem::ManuallyDrop,
	ptr::NonNull,
	slice,
};

use funty::IsInteger;

use tap::{
	pipe::Pipe,
	tap::Tap,
};

/** A vector of individual bits, allocated on the heap.

This is a managed, heap-allocated, buffer that contains a `BitSlice` region. It
is analagous to `Vec<bool>`, and is written to be as close as possible to
drop-in replacabale for it. This type contains little interesting behavior in
its own right, dereferencing instead to [`BitSlice`] for manipulation of the
buffer contents, and serves primarily as an interface to the allocator. If you
require statically-allocated, fixed-size, owned buffers, you should use the
[`BitArray`] type.

Because `BitVec` directly owns its memory, and can guarantee that no other
object in a program has access to its buffers, `BitVec` is able to override some
behavior from `BitSlice` in more efficient manners.

# Documentation

All APIs that mirror something in the standard library will have an `Original`
section linking to the corresponding item. All APIs that have a different
signature or behavior than the original will have an `API Differences` section
explaining what has changed, and how to adapt your existing code to the change.

These sections look like this:

# Original

[`Vec<T>`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html)

# API Differences

The buffer type `Vec<bool>` has no type parameters. `BitVec<O, T>` has the same
two type parameters as `BitSlice<O, T>`. Otherwise, `BitVec` is able to
implement the full API surface of `Vec<bool>`.

# Behavior

Because `BitVec` is a fully-owned buffer, it is able to operate on its memory
without concern for any other views that may alias. This enables it to
specialize some `BitSlice` behavior to be faster or more efficient.

# Type Parameters

This takes the same two type parameters, `O: BitOrder` and `T: BitStore`, as
`BitSlice`.

# Safety

Like `BitSlice`, `BitVec` is exactly equal in size to `Vec`, and is also
absolutely representation-incompatible with it. You must never attempt to
type-cast between `Vec<T>` and `BitVec` in any way, nor attempt to modify the
memory value of a `BitVec` handle. Doing so will cause allocator and memory
errors in your program, likely inducing a panic.

Everything in the `BitVec` public API, even the `unsafe` parts, are guaranteed
to have no more unsafety than their equivalent items in the standard library.
All `unsafe` APIs will have documentation explicitly detailing what the API
requires you to uphold in order for it to function safely and correctly. All
safe APIs will do so themselves.

# Performance

The choice of `T: BitStore` type parameter can impact your vector’s performance,
as the allocator operates in units of `T` rather than in bits. This means that
larger register types will increase the amount of memory reserved in each call
to the allocator, meaning fewer calls to `.push()` will actually cause a
reällocation. In addition, iteration over the vector is governed by the
`BitSlice` characteristics on the type parameter. You are generally better off
using larger types when your vector is a data collection rather than a specific
I/O protocol buffer.

# Macro Construction

Heap allocation can only occur at runtime, but the [`bitvec!`] macro will
construct an appropriate `BitSlice` buffer at compile-time, and at run-time,
only copy the buffer into a heap allocation.

[`BitArray`]: ../array/struct.BitArray.html
[`BitSlice`]: ../slice/struct.BitSlice.html
[`bitvec!`]: ../macro.bitvec.html
**/
#[repr(C)]
pub struct BitVec<O = Lsb0, T = usize>
where
	O: BitOrder,
	T: BitStore,
{
	/// Region pointer describing the live portion of the owned buffer.
	pointer: NonNull<BitSlice<O, T>>,
	/// Allocated capacity, in elements `T`, of the owned buffer.
	capacity: usize,
}

/// Methods specific to `BitVec<_, T>`, and not present on `Vec<T>`.
impl<O, T> BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Constructs a `BitVec` from a value repeated many times.
	///
	/// This function is equivalent to the `bitvec![O, T; bit; len]` macro call,
	/// and is in fact the implementation of that macro syntax.
	///
	/// # Parameters
	///
	/// - `bit`: The bit value to which all `len` allocated bits will be set.
	/// - `len`: The number of live bits in the constructed `BitVec`.
	///
	/// # Returns
	///
	/// A `BitVec` with `len` live bits, all set to `bit`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = BitVec::<Msb0, u8>::repeat(true, 20);
	/// assert_eq!(bv, bits![1; 20]);
	/// ```
	#[inline]
	pub fn repeat(bit: bool, len: usize) -> Self {
		let mut out = Self::with_capacity(len);
		unsafe {
			out.set_len(len);
		}
		out.set_elements(if bit { T::Mem::ALL } else { T::Mem::ZERO });
		out
	}

	/// Clones a `&BitSlice` into a `BitVec`.
	///
	/// # Original
	///
	/// [`<Vec<T: Clone> as Clone>::clone`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#impl-Clone)
	///
	/// # Effects
	///
	/// This performs a direct element-wise copy from the source slice to the
	/// newly-allocated buffer, then sets the vector to have the same starting
	/// bit as the slice did. This allows for faster behavior. If you require
	/// that the vector start at the leading edge of the first element, use
	/// [`force_align`] to guarantee this.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 1, 1, 0, 1, 1];
	/// let bv = BitVec::from_bitslice(&bits[2 ..]);
	/// assert_eq!(bv, bits[2 ..]);
	/// ```
	///
	/// [`force_align`]: #method.force_align
	#[inline]
	pub fn from_bitslice(slice: &BitSlice<O, T>) -> Self {
		let mut bitptr = slice.bitptr();
		let (base, elts) = (bitptr.pointer().to_const(), bitptr.elements());
		let source = unsafe { slice::from_raw_parts(base, elts) };

		let vec = elts
			.pipe(Vec::with_capacity)
			.pipe(ManuallyDrop::new)
			.tap_mut(|v| v.extend(source.iter().map(BitStore::load_value)));

		unsafe {
			bitptr.set_pointer(vec.as_ptr() as *const T);
		}

		let capacity = vec.capacity();
		Self {
			pointer: bitptr.to_nonnull(),
			capacity,
		}
	}

	/// Converts a `Vec<T>` into a `BitVec<O, T>` without copying its buffer.
	///
	/// # Parameters
	///
	/// - `vec`: A vector to view as bits.
	///
	/// # Returns
	///
	/// A `BitVec` over the `vec` buffer.
	///
	/// # Panics
	///
	/// This panics if `vec` is too long to convert into a `BitVec`. See
	/// [`BitSlice::MAX_ELTS`].
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let vec = vec![0u8; 4];
	/// let bv = BitVec::<LocalBits, _>::from_vec(vec);
	/// assert_eq!(bv, bits![0; 32]);
	/// ```
	///
	/// [`BitSlice::MAX_ELTS`]:
	/// ../slice/struct.BitSlice.html#associatedconstant.MAX_ELTS
	#[inline]
	pub fn from_vec(vec: Vec<T>) -> Self {
		Self::try_from_vec(vec)
			.expect("Vector was too long to be converted into a `BitVec`")
	}

	/// Converts a `Vec<T>` into a `BitVec<O, T>` without copying its buffer.
	///
	/// This method takes ownership of a memory buffer and enables it to be used
	/// as a bit-vector. Because `Vec` can be longer than `BitVec`s, this is a
	/// fallible method, and the original vector will be returned if it cannot
	/// be converted.
	///
	/// # Parameters
	///
	/// - `vec`: Some vector of memory, to be viewed as bits.
	///
	/// # Returns
	///
	/// If `vec` is short enough to be viewed as a `BitVec`, then this returns
	/// a `BitVec` over the `vec` buffer. If `vec` is too long, then this
	/// returns `vec` unmodified.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let vec = vec![0u8; 4];
	/// let bv = BitVec::<LocalBits, _>::try_from_vec(vec).unwrap();
	/// assert_eq!(bv, bits![0; 32]);
	/// ```
	///
	/// An example showing this function failing would require an allocation
	/// exceeding `!0usize >> 3` bytes in size, which is infeasible to produce.
	#[inline]
	pub fn try_from_vec(vec: Vec<T>) -> Result<Self, Vec<T>> {
		let len = vec.len();
		if len > BitSlice::<O, T>::MAX_ELTS {
			return Err(vec);
		}

		let vec = ManuallyDrop::new(vec);
		let (base, capacity) = (vec.as_ptr(), vec.capacity());
		Ok(Self {
			pointer: unsafe {
				BitPtr::new_unchecked(
					base,
					BitIdx::ZERO,
					len * T::Mem::BITS as usize,
				)
			}
			.to_nonnull(),
			capacity,
		})
	}

	/// Copies all bits in a `BitSlice` into the `BitVec`.
	///
	/// This is provided for API completeness; it has no performance benefits
	/// compared to use of the [`Extend`] implementation.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `other`: A `BitSlice` reference of the same type parameters as `self`.
	///
	/// # Behavior
	///
	/// `self` is extended by the length of `other`, and then the contents of
	/// `other` are copied into the newly-allocated end of `self`.
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1];
	/// bv.extend_from_bitslice(bits![1, 1, 0, 1]);
	///
	/// assert_eq!(bv, bits![0, 1, 1, 1, 0, 1]);
	/// ```
	///
	/// [`Extend`]: #impl-Extend<%26'a bool>
	/// [`.as_bitslice()`]: #method.as_bitslice()
	#[inline]
	pub fn extend_from_bitslice(&mut self, other: &BitSlice<O, T>) {
		let len = self.len();
		let olen = other.len();
		self.resize(len + olen, false);
		unsafe { self.get_unchecked_mut(len ..) }.clone_from_bitslice(other);
	}

	/// Converts the vector into [`BitBox<O, T>`].
	///
	/// Note that this will drop any excess capacity.
	///
	/// # Original
	///
	/// [`Vec::into_boxed_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.into_boxed_slice)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![1; 50];
	/// let bb: BitBox = bv.into_boxed_bitslice();
	/// assert_eq!(bb, bits![1; 50]);
	/// ```
	///
	/// [`BitBox<O, T>`]: ../boxed/struct.BitBox.html
	#[inline]
	pub fn into_boxed_bitslice(self) -> BitBox<O, T> {
		let mut bitptr = self.bitptr();
		let boxed = self.into_boxed_slice().pipe(ManuallyDrop::new);
		unsafe {
			bitptr.set_pointer(boxed.as_ptr());
		}
		unsafe { BitBox::from_raw(bitptr.to_bitslice_ptr_mut::<O>()) }
	}

	/// Converts the vector back into an ordinary vector of memory elements.
	///
	/// This does not affect the vector’s buffer, only the handle used to
	/// control it.
	///
	/// # Parameters
	///
	/// - `self`
	///
	/// # Returns
	///
	/// An ordinary vector containing all of the bit-vector’s memory buffer.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![0; 5];
	/// let vec = bv.into_vec();
	/// assert_eq!(vec, [0]);
	/// ```
	#[inline]
	pub fn into_vec(self) -> Vec<T> {
		let mut this = ManuallyDrop::new(self);
		let buf = this.as_mut_slice();
		unsafe {
			Vec::from_raw_parts(
				buf.as_mut_ptr() as *mut T,
				buf.len(),
				this.capacity,
			)
		}
	}

	/// Gets the number of elements `T` that contain live bits of the vector.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![LocalBits, u16; 1; 50];
	/// assert_eq!(bv.elements(), 4);
	/// ```
	#[inline]
	pub fn elements(&self) -> usize {
		self.bitptr().elements()
	}

	/// Sets the uninitialized bits of the vector to a fixed value.
	///
	/// This method modifies all bits in the allocated buffer that are outside
	/// the `self.as_bitslice()` view so that they have a consistent value. This
	/// can be used to zero the uninitialized memory so that when viewed as a
	/// raw memory slice, bits outside the live region have a predictable value.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = 220u8.view_bits::<Lsb0>().to_bitvec();
	/// assert_eq!(bv.as_slice(), &[220u8]);
	/// bv.truncate(4);
	/// assert_eq!(bv.count_ones(), 2);
	/// assert_eq!(bv.as_slice(), &[220u8]);
	///
	/// bv.set_uninitialized(false);
	/// assert_eq!(bv.as_slice(), &[12u8]);
	///
	/// bv.set_uninitialized(true);
	/// assert_eq!(bv.as_slice(), &[!3u8]);
	/// ```
	#[inline]
	pub fn set_uninitialized(&mut self, value: bool) {
		let head = self.bitptr().head().value() as usize;
		let tail = head + self.len();
		let capa = self.capacity();
		let mut bp = self.bitptr();
		unsafe {
			bp.set_head(BitIdx::ZERO);
			bp.set_len(capa);
			let bits = bp.to_bitslice_mut::<O>();
			bits.get_unchecked_mut(.. head).set_all(value);
			bits.get_unchecked_mut(tail ..).set_all(value);
		}
	}

	/// Ensures that the live region of the vector’s contents begins at the
	/// leading edge of the buffer.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0x3Cu8;
	/// let bits = data.view_bits::<Msb0>();
	///
	/// let mut bv = bits[2 .. 6].to_bitvec();
	/// assert_eq!(bv, bits[2 .. 6]);
	/// assert_eq!(bv.as_slice()[0], data);
	///
	/// bv.force_align();
	/// assert_eq!(bv, bits[2 .. 6]);
	/// //  It is not specified what happens to bits that are no longer used.
	/// assert_eq!(bv.as_slice()[0] & 0xF0, 0xF0);
	/// ```
	#[inline]
	pub fn force_align(&mut self) {
		let bitptr = self.bitptr();
		let head = bitptr.head().value() as usize;
		if head == 0 {
			return;
		}
		let last = bitptr.len() + head;
		unsafe {
			self.pointer =
				bitptr.tap_mut(|bp| bp.set_head(BitIdx::ZERO)).to_nonnull();
			self.copy_within_unchecked(head .. last, 0);
		}
	}

	/// Writes a value into every element that the vector considers live.
	///
	/// This unconditionally writes `element` into each live location in the
	/// backing buffer, without altering the `BitVec`’s length or capacity.
	///
	/// It is unspecified what effects this has on the allocated but dead
	/// elements in the buffer.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `element`: The value which will be written to each live location in
	///   the vector’s buffer.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![LocalBits, u8; 0; 10];
	/// assert_eq!(bv.as_slice(), [0, 0]);
	/// bv.set_elements(0xA5);
	/// assert_eq!(bv.as_slice(), [0xA5, 0xA5]);
	/// ```
	#[inline]
	pub fn set_elements(&mut self, element: T::Mem) {
		self.as_mut_slice()
			.iter_mut()
			.for_each(|elt| *elt = element.into());
	}

	/// Views the buffer’s contents as a `BitSlice`.
	///
	/// This is equivalent to `&bv[..]`.
	///
	/// # Original
	///
	/// [`Vec::as_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_slice)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![0, 1, 1, 0];
	/// let bits = bv.as_bitslice();
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_bitslice(&self) -> &BitSlice<O, T> {
		unsafe { &*self.pointer.as_ptr() }
	}

	/// Extracts a mutable bit-slice of the entire vector.
	///
	/// Equivalent to `&mut bv[..]`.
	///
	/// # Original
	///
	/// [`Vec::as_mut_slice`](https://doc.rust-lang.org/alloc/vec/struct.Vec.html#method.as_mut_slice)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0, 1, 0, 1];
	/// let bits = bv.as_mut_bitslice();
	/// bits.set(0, true);
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_bitslice(&mut self) -> &mut BitSlice<O, T> {
		unsafe { &mut *self.pointer.as_ptr() }
	}

	/// Returns a raw pointer to the vector’s region.
	///
	/// The caller must ensure that the vector outlives the pointer this
	/// function returns, or else it will end up pointing to garbage. Modifying
	/// the vector may cause its buffer to be reallocated, which would also make
	/// any pointers to it invalid.
	///
	/// The caller must also ensure that the memory the pointer
	/// (non-transitively) points to is never written to (except inside an
	/// `UnsafeCell`) using this pointer or any pointer derived from it. If you
	/// need to mutate the contents of the region, use [`as_mut_bitptr`].
	///
	/// This pointer is an opaque crate-internal type. Its in-memory
	/// representation is unsafe to modify in any way. The only safe action to
	/// take with this pointer is to pass it, unchanged, back into a `bitvec`
	/// API.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bv = bitvec![0; 20];
	/// let ptr = bv.as_bitptr();
	///
	/// let bits = unsafe { &*ptr };
	/// assert_eq!(bv, bits);
	/// ```
	///
	/// [`as_mut_bitptr`]: #method.as_mut_bitptr
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_bitptr(&self) -> *const BitSlice<O, T> {
		self.pointer.as_ptr() as *const BitSlice<O, T>
	}

	/// Returns an unsafe mutable pointer to the vector’s region.
	///
	/// The caller must ensure that the vector outlives the pointer this
	/// function returns, or else it will end up pointing to garbage. Modifying
	/// the vector may cause its buffer to be reallocated, which would also make
	/// any pointers to it invalid.
	///
	/// This pointer is an opaque crate-internal type. Its in-memory
	/// representation is unsafe to modify in any way. The only safe action to
	/// take with this pointer is to pass it, unchanged, back into a `bitvec`
	/// API.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut bv = bitvec![0; 20];
	/// let ptr = bv.as_mut_bitptr();
	///
	/// let bits = unsafe { &mut *ptr };
	/// assert_eq!(bv, bits);
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_bitptr(&mut self) -> *mut BitSlice<O, T> {
		self.pointer.as_ptr()
	}

	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub(crate) fn bitptr(&self) -> BitPtr<T> {
		self.pointer.as_ptr().pipe(BitPtr::from_bitslice_ptr_mut)
	}

	/// Permits a function to modify the `Vec<T>` backing storage of a
	/// `BitVec<_, T>`.
	///
	/// This produces a temporary `Vec<T::Mem>` structure governing the
	/// `BitVec`’s buffer and allows a function to view it mutably. After the
	/// callback returns, the `Vec` is written back into `self` and forgotten.
	///
	/// # Type Parameters
	///
	/// - `F`: A function which operates on a mutable borrow of a `Vec<T::Mem>`
	///   buffer controller.
	/// - `R`: The return type of the `F` function.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `func`: A function which receives a mutable borrow of a `Vec<T::Mem>`
	///   controlling `self`’s buffer.
	///
	/// # Returns
	///
	/// The return value of `func`. `func` is forbidden from borrowing any part
	/// of the `Vec<T::Mem>` temporary view.
	fn with_vec<F, R>(&mut self, func: F) -> R
	where F: FnOnce(&mut ManuallyDrop<Vec<T::Mem>>) -> R {
		let cap = self.capacity;
		let mut bitptr = self.bitptr();
		let (base, elts) =
			(bitptr.pointer().to_mut() as *mut T::Mem, bitptr.elements());

		let mut vec = unsafe { Vec::from_raw_parts(base, elts, cap) }
			.pipe(ManuallyDrop::new);
		let out = func(&mut vec);

		unsafe {
			bitptr.set_pointer(vec.as_ptr() as *mut T);
		}
		self.pointer = bitptr.to_nonnull();
		self.capacity = vec.capacity();
		out
	}
}

mod api;
mod iter;
mod ops;
mod traits;

pub use iter::{
	Drain,
	IntoIter,
	Splice,
};

#[cfg(test)]
mod tests;
