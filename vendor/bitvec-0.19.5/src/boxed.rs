/*! A dynamically-allocated, fixed-size, buffer containing a `BitSlice` region.

You can read the standard library’s [`alloc::boxed` module documentation][std]
here.

This module defines the [`BitBox`] buffer, and all of its associated support
code.

`BitBox` is equivalent to `Box<[bool]>`, in its operation and in its
relationship to the `BitSlice` and [`BitVec`] types. Most of the interesting
work to be done on a bit-sequence is implemented in `BitSlice`, to which
`BitBox` dereferences, and the box container itself only exists to maintain
wonership and provide some specializations that cannot safely be done on
`BitSlice` alone.

There is almost never a reason to use this type, as it is a mixture of
[`BitArray`]’s fixed width and [`BitVec`]’s heap allocation. You should only use
it when you have a bit-sequence whose width is either unknowable at compile-time
or inexpressable in `BitArray`, and are constructing the sequence in a `BitVec`
before freezing it.

[`BitArray`]: ../array/struct.BitArray.html
[`BitBox`]: struct.BitBox.html
[`BitSlice`]: ../slice/struct.BitSlice.html
[`BitVec`]: ../vec/struct.BitVec.html
[std]: https://doc.rust-lang.org/alloc/boxed
!*/

#![cfg(feature = "alloc")]

use crate::{
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

use alloc::boxed::Box;

use core::{
	mem::ManuallyDrop,
	ptr::NonNull,
	slice,
};

use tap::pipe::Pipe;

/** A frozen heap-allocated buffer of individual bits.

This is essentially a [`BitVec`] that has frozen its allocation, and given up
the ability to change size. It is analagous to `Box<[bool]>`, and is written to
be as close as possible to drop-in replacable for it. This type contains almost
no interesting behavior in its own right; it dereferences to [`BitSlice`] to
manipulate its contents, and it converts to and from `BitVec` for allocation
control.

If you know the length of your bit sequence at compile-time, and it is
expressible within the limits of [`BitArray`], you should prefer that type
instead. Large `BitArray`s can be `Box`ed normally as desired.

# Documentation

All APIs that mirror something in the standard library will have an `Original`
section linking to the corresponding item. All APIs that have a different
signature or behavior than the original will have an `API Differences` section
explaining what has changed, and how to adapt your existing code to the change.

These sections look like this:

# Original

[`Box<[T]>`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html)

# API Differences

The buffer type `Box<[bool]>` has no type parameters. `BitBox<O, T>` has the
same two type parameters as `BitSlice<O, T>`. Otherwise, `BitBox` is able to
implement the full API surface of `Box<[bool]>`.

# Behavior

Because `BitBox` is a fully-owned buffer, it is able to operate on its memory
without concern for any other views that may alias. This enables it to
specialize some `BitSlice` behavior to be faster or more efficient.

# Type Parameters

This takes the same two type parameters, `O: BitOrder` and `T: BitStore`, as
`BitSlice`.

# Safety

Like `BitSlice`, `BitBox` is exactly equal in size to `Box<[bool]>`, and is also
absolutely representation-incompatible with it. You must never attempt to
type-cast between `Box<[bool]>` and `BitBox` in any way, nor attempt to modify
the memory value of a `BitBox` handle. Doing so will cause allocator and memory
errors in your program, likely inducing a panic.

Everything in the `BitBox` public API, even the `unsafe` parts, are guaranteed
to have no more unsafety than their equivalent items in the standard library.
All `unsafe` APIs will have documentation explicitly detailing what the API
requires you to uphold in order for it to function safely and correctly. All
safe APIs will do so themselves.

# Performance

Iteration over the buffer is governed by the `BitSlice` characteristics on the
type parameter. You are generally better off using larger types when your buffer
is a data collection rather than a specific I/O protocol buffer.

# Macro Construction

Heap allocation can only occur at runtime, but the [`bitbox!`] macro will
construct an appropriate `BitSlice` buffer at compile-time, and at run-time,
only copy the buffer into a heap allocation.

[`BitArray`]: ../array/struct.BitArray.html
[`BitSlice`]: ../slice/struct.BitSlice.html
[`BitVec`]: ../vec/struct.BitVec.html
[`bitbox!`]: ../macro.bitbox.html
**/
#[repr(transparent)]
pub struct BitBox<O = Lsb0, T = usize>
where
	O: BitOrder,
	T: BitStore,
{
	pointer: NonNull<BitSlice<O, T>>,
}

/// Methods specific to `BitBox<_, T>`, and not present on `Box<[T]>`.
impl<O, T> BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Clones a `&BitSlice` into a `BitVec`.
	///
	/// # Original
	///
	/// [`<Box<T: Clone> as Clone>::clone`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html#impl-Clone)
	///
	/// # Effects
	///
	/// This performs a direct element-wise copy from the source slice to the
	/// newly-allocated buffer, then sets the box to have the same starting bit
	/// as the slice did. This allows for faster behavior.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 1, 1, 0, 1, 1];
	/// let bb = BitBox::from_bitslice(&bits[2 ..]);
	/// assert_eq!(bb, bits[2 ..]);
	/// ```
	#[inline]
	pub fn from_bitslice(slice: &BitSlice<O, T>) -> Self {
		slice.to_bitvec().into_boxed_bitslice()
	}

	/// Converts a `Box<[T]>` into a `BitBox`<O, T>` without copying its buffer.
	///
	/// # Parameters
	///
	/// - `boxed`: A boxed slice to view as bits.
	///
	/// # Returns
	///
	/// A `BitBox` over the `boxed` buffer.
	///
	/// # Panics
	///
	/// This panics if `boxed` is too long to convert into a `BitBox`. See
	/// [`BitSlice::MAX_ELTS`].
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let boxed: Box<[u8]> = Box::new([0; 4]);
	/// let bb = BitBox::<LocalBits, _>::from_boxed_slice(boxed);
	/// assert_eq!(bb, bits![0; 32]);
	/// ```
	///
	/// [`BitSlice::MAX_ELTS`]:
	/// ../slice/struct.BitSlice.html#associatedconstant.MAX_ELTS
	#[inline]
	pub fn from_boxed_slice(boxed: Box<[T]>) -> Self {
		Self::try_from_boxed_slice(boxed)
			.expect("Slice was too long to be converted into a `BitBox`")
	}

	/// Converts a `Box<[T]>` into a `BitBox<O, T>` without copying its buffer.
	///
	/// This method takes ownership of a memory buffer and enables it to be used
	/// as a bit-box. Because `Box<[T]>` can be longer than `BitBox`es, this is
	/// a fallible method, and the original box will be returned if it cannot be
	/// converted.
	///
	/// # Parameters
	///
	/// - `boxed`: Some boxed slice of memory, to be viewed as bits.
	///
	/// # Returns
	///
	/// If `boxed` is short enough to be viewed as a `BitBox`, then this returns
	/// a `BitBox` over the `boxed` buffer. If `boxed` is too long, then this
	/// returns `boxed` unmodified.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let boxed: Box<[u8]> = Box::new([0; 4]);
	/// let bb = BitBox::<LocalBits, _>::try_from_boxed_slice(boxed).unwrap();
	/// assert_eq!(bb[..], bits![0; 32]);
	/// ```
	#[inline]
	pub fn try_from_boxed_slice(boxed: Box<[T]>) -> Result<Self, Box<[T]>> {
		let len = boxed.len();
		if len > BitSlice::<O, T>::MAX_ELTS {
			return Err(boxed);
		}

		let boxed = ManuallyDrop::new(boxed);
		let base = boxed.as_ptr();
		Ok(Self {
			pointer: unsafe {
				BitPtr::new_unchecked(
					base,
					BitIdx::ZERO,
					len * T::Mem::BITS as usize,
				)
			}
			.to_nonnull(),
		})
	}

	/// Converts the slice back into an ordinary slice of memory elements.
	///
	/// This does not affect the slice’s buffer, only the handle used to control
	/// it.
	///
	/// # Parameters
	///
	/// - `self`
	///
	/// # Returns
	///
	/// An ordinary boxed slice containing all of the bit-slice’s memory buffer.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bb = bitbox![0; 5];
	/// let boxed = bb.into_boxed_slice();
	/// assert_eq!(boxed[..], [0][..]);
	/// ```
	#[inline]
	pub fn into_boxed_slice(self) -> Box<[T]> {
		let mut this = ManuallyDrop::new(self);
		unsafe { Box::from_raw(this.as_mut_slice()) }
	}

	/// Views the buffer’s contents as a `BitSlice`.
	///
	/// This is equivalent to `&bb[..]`.
	///
	/// # Original
	///
	/// [`<Box<[T]> as AsRef<[T]>>::as_ref`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html#impl-AsRef%3CT%3E)
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bb = bitbox![0, 1, 1, 0];
	/// let bits = bb.as_bitslice();
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_bitslice(&self) -> &BitSlice<O, T> {
		self.bitptr().to_bitslice_ref()
	}

	/// Extracts a mutable bit-slice of the entire vector.
	///
	/// Equivalent to `&mut bv[..]`.
	///
	/// # Original
	///
	/// [`<Box<[T]> as AsMut<[T]>>::as_mut`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html#impl-AsMut%3CT%3E)
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
		self.bitptr().to_bitslice_mut()
	}

	/// Extracts an element slice containing the entire box.
	///
	/// # Original
	///
	/// [`<Box<[T]> as AsRef<[T]>>::as_ref`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html#impl-AsRef%3CT%3E)
	///
	/// # Analogue
	///
	/// See [`as_bitslice`] for a `&BitBox -> &BitSlice` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	/// use std::io::{self, Write};
	/// let buffer = bitbox![Msb0, u8; 0, 1, 0, 1, 1, 0, 0, 0];
	/// io::sink().write(buffer.as_slice()).unwrap();
	/// # }
	/// ```
	///
	/// [`as_bitslice`]: #method.as_bitslice
	#[inline]
	pub fn as_slice(&self) -> &[T] {
		let bitptr = self.bitptr();
		let (base, elts) = (bitptr.pointer().to_const(), bitptr.elements());
		unsafe { slice::from_raw_parts(base, elts) }
	}

	/// Extracts a mutable slice of the entire box.
	///
	/// # Original
	///
	/// [`<Box<[T]> as AsMut<[T]>>::as_mut`](https://doc.rust-lang.org/alloc/boxed/struct.Box.html#impl-AsMut%3CT%3E)
	///
	/// # Analogue
	///
	/// See [`as_mut_bitslice`] for a `&mut BitBox -> &mut BitSlice` transform.
	///
	/// # Examples
	///
	/// ```rust
	/// # #[cfg(feature = "std")] {
	/// use bitvec::prelude::*;
	/// use std::io::{self, Read};
	/// let mut buffer = bitbox![Msb0, u8; 0; 24];
	/// io::repeat(0b101).read_exact(buffer.as_mut_slice()).unwrap();
	/// # }
	/// ```
	///
	/// [`as_mut_bitslice`]: #method.as_mut_bitslice
	#[inline]
	pub fn as_mut_slice(&mut self) -> &mut [T] {
		let bitptr = self.bitptr();
		let (base, elts) = (bitptr.pointer().to_mut(), bitptr.elements());
		unsafe { slice::from_raw_parts_mut(base, elts) }
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
	/// let mut bb = BitBox::new(&220u8.view_bits::<Lsb0>()[.. 4]);
	/// assert_eq!(bb.count_ones(), 2);
	/// assert_eq!(bb.as_slice(), &[220u8]);
	///
	/// bb.set_uninitialized(false);
	/// assert_eq!(bb.as_slice(), &[12u8]);
	///
	/// bb.set_uninitialized(true);
	/// assert_eq!(bb.as_slice(), &[!3u8]);
	/// ```
	#[inline]
	pub fn set_uninitialized(&mut self, value: bool) {
		let head = self.bitptr().head().value() as usize;
		let tail = head + self.len();
		let elts = self.bitptr().elements() * T::Mem::BITS as usize;
		let mut bp = self.bitptr();
		unsafe {
			bp.set_head(BitIdx::ZERO);
			bp.set_len(elts);
			let bits = bp.to_bitslice_mut::<O>();
			bits.get_unchecked_mut(.. head).set_all(value);
			bits.get_unchecked_mut(tail ..).set_all(value);
		}
	}

	#[inline]
	pub(crate) fn bitptr(&self) -> BitPtr<T> {
		self.pointer.as_ptr().pipe(BitPtr::from_bitslice_ptr_mut)
	}

	/// Permits a function to modify the `Box<[T]>` backing storage of a
	/// `BitBox<_, T>`.
	///
	/// This produces a temporary `Box<[T::Mem]>` structure governing the
	/// `BitBox`’s buffer and allows a function to view it mutably. After the
	/// callback returns, the `Box` is written back into `self` and forgotten.
	///
	/// # Type Parameters
	///
	/// - `F`: A function which operates on a mutable borrow of a
	///   `Box<[T::Mem]>` buffer controller.
	/// - `R`: The return type of the `F` function.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `func`: A function which receives a mutable borrow of a
	///   `Box<[T::Mem]>` controlling `self`’s buffer.
	///
	/// # Returns
	///
	/// The return value of `func`. `func` is forbidden from borrowing any part
	/// of the `Box<[T::Mem]>` temporary view.
	fn with_box<F, R>(&mut self, func: F) -> R
	where F: FnOnce(&mut ManuallyDrop<Box<[T::Mem]>>) -> R {
		self.as_mut_slice()
			.pipe(|s| s as *mut [T] as *mut [T::Mem])
			.pipe(|raw| unsafe { Box::from_raw(raw) })
			.pipe(ManuallyDrop::new)
			.pipe_ref_mut(func)
	}
}

mod api;
mod ops;
mod traits;

#[cfg(test)]
mod tests;
