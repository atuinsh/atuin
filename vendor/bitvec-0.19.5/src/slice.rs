/*! A dynamically-sized view into individual bits of a memory region.

You can read the language’s [`slice` module documentation][std] here.

This module defines the [`BitSlice`] region, and all of its associated support
code.

`BitSlice` is the primary working type of this crate. It is a wrapper type over
`[T]` which enables you to view, manipulate, and take the address of individual
bits in memory. It behaves in every possible respect exactly like an ordinary
slice: it is dynamically-sized, and must be held by `&` or `&mut` reference,
just like `[T]`, and implements every inherent method and trait that `[T]` does,
to the absolute limits of what Rust permits.

The key to `BitSlice`’s powerful capability is that references to it use a
special encoding that store, in addition to the address of the base element and
the bit length, the index of the starting bit in the base element. This custom
reference encoding has some costs in what APIs are possible – for instance, Rust
forbids it from supporting `&mut BitSlice[index] = bool` write indexing – but in
exchange, enables it to be *far* more capable than any other bit-slice crate in
existence.

Because of the volume of code that must be written to match the `[T]` standard
API, this module is organized very differently than the slice implementation in
the `core` and `std` distribution libraries.

- the root module `slice` contains new APIs that have no counterpart in `[T]`
- `slice/api` contains reïmplementations of the `[T]` inherent methods
- `slice/iter` implements all of the iteration capability
- `slice/ops` implements the traits in `core::ops`
- `slice/proxy` implements the proxy reference used in place of `&mut bool`
- `slice/traits` implements all other traits not in `core::ops`
- lastly, `slice/tests` contains all the unit tests.

[`BitSlice`]: struct.BitSlice.html
[std]: https://doc.rust-lang.org/std/slice
!*/

use crate::{
	access::BitAccess,
	devel as dvl,
	domain::{
		BitDomain,
		BitDomainMut,
		Domain,
		DomainMut,
	},
	index::{
		BitIdx,
		BitMask,
		BitRegister,
	},
	mem::BitMemory,
	order::{
		BitOrder,
		Lsb0,
	},
	pointer::BitPtr,
	store::BitStore,
};

use core::{
	cmp,
	marker::PhantomData,
	ops::RangeBounds,
	ptr,
	slice,
};

use funty::IsInteger;

use radium::Radium;

use tap::pipe::Pipe;

/** A slice of individual bits, anywhere in memory.

This is the main working type of the crate. It is analagous to `[bool]`, and is
written to be as close as possible to drop-in replacable for it. This type
contains most of the *methods* used to operate on memory, but it will rarely be
named directly in your code. You should generally prefer to use [`BitArray`] for
fixed-size arrays or [`BitVec`] for dynamic vectors, and use `&BitSlice`
references only where you would directly use `&[bool]` or `&[u8]` references
before using this crate.

As it is a slice wrapper, you are intended to work with this through references
(`&BitSlice<O, T>` and `&mut BitSlice<O, T>`) or through the other data
structures provided by `bitvec` that are implemented atop it. Once created,
references to `BitSlice` are guaranteed to work just like references to `[bool]`
to the fullest extent possible in the Rust language.

Every bit-vector crate can give you an opaque type that hides shift/mask
operations from you. `BitSlice` does far more than this: it offers you the full
Rust guarantees about reference behavior, including lifetime tracking,
mutability and aliasing awareness, and explicit memory control, *as well as* the
full set of tools and APIs available to the standard `[bool]` slice type.
`BitSlice` can arbitrarily split and subslice, just like `[bool]`. You can write
a linear consuming function and keep the patterns already know.

For example, to trim all the bits off either edge that match a condition, you
could write

```rust
use bitvec::prelude::*;

fn trim<O: BitOrder, T: BitStore>(
  bits: &BitSlice<O, T>,
  to_trim: bool,
) -> &BitSlice<O, T> {
  let stop = |b: &bool| *b != to_trim;
  let front = bits.iter().position(stop).unwrap_or(0);
  let back = bits.iter().rposition(stop).unwrap_or(0);
  &bits[front ..= back]
}
# assert_eq!(trim(bits![0, 0, 1, 1, 0, 1, 0], false), bits![1, 1, 0, 1]);
```

to get behavior something like
`trim(&BitSlice[0, 0, 1, 1, 0, 1, 0], false) == &BitSlice[1, 1, 0, 1]`.

# Documentation

All APIs that mirror something in the standard library will have an `Original`
section linking to the corresponding item. All APIs that have a different
signature or behavior than the original will have an `API Differences` section
explaining what has changed, and how to adapt your existing code to the change.

These sections look like this:

# Original

[`slice`](https://doc.rust-lang.org/std/primitive.slice.html)

# API Differences

The slice type `[bool]` has no type parameters. `BitSlice<O, T>` has two: one
for the memory type used as backing storage, and one for the order of bits
within that memory type.

`&BitSlice<O, T>` is capable of producing `&bool` references to read bits out
of its memory, but is not capable of producing `&mut bool` references to write
bits *into* its memory. Any `[bool]` API that would produce a `&mut bool` will
instead produce a [`BitMut<O, T>`] proxy reference.

# Behavior

`BitSlice` is a wrapper over `[T]`. It describes a region of memory, and must be
handled indirectly. This is most commonly through the reference types
`&BitSlice` and `&mut BitSlice`, which borrow memory owned by some other value
in the program. These buffers can be directly owned by the sibling types
`BitBox`, which behavios like `Box<[T]>`, and `BitVec`, which behaves like
`Vec<T>`. It cannot be used as the type parameter to a standard-library-provided
handle type.

The `BitSlice` region provides access to each individual bit in the region, as
if each bit had a memory address that you could use to dereference it. It packs
each logical bit into exactly one bit of storage memory, just like
[`std::bitset`] and [`std::vector<bool>`] in C++.

# Type Parameters

`BitSlice` has two type parameters which propagate through nearly every public
API in the crate. These are very important to its operation, and your choice
of type arguments informs nearly every part of this library’s behavior.

## `T: BitStore`

This is the simpler of the two parameters. It refers to the integer type used to
hold bits. It must be one of the Rust unsigned integer fundamentals: `u8`,
`u16`, `u32`, `usize`, and on 64-bit systems only, `u64`. In addition, it can
also be the `Cell<N>` wrapper over any of those, or their equivalent types in
`core::sync::atomic`. Unless you know you need to have `Cell` or atomic
properties, though, you should use a plain integer.

The default type argument is `usize`.

The argument you choose is used as the basis of a `[T]` slice, over which the
`BitSlice` view type is placed. `BitSlice<_, T>` is subject to all of the rules
about alignment that `[T]` is. If you are working with in-memory representation
formats, chances are that you already have a `T` type with which you’ve been
working, and should use it here.

If you are only using this crate to discard the seven wasted bits per `bool`
of a collection of `bool`s, and are not too concerned about the in-memory
representation, then you should use the default type argument of `usize`. This
is because most processors work best when moving an entire `usize` between
memory and the processor itself, and using a smaller type may cause it to slow
down.

## `O: BitOrder`

This is the more complex parameter. It has a default argument which, like
`usize`, is the good-enough choice when you do not explicitly need to control
the representation of bits in memory.

This parameter determines how to index the bits within a single memory element
`T`. Computers all agree that in a slice of elements `T`, the element with the
lower index has a lower memory address than the element with the higher index.
But the individual bits within an element do not have addresses, and so there is
no uniform standard of which bit is the zeroth, which is the first, which is the
penultimate, and which is the last.

To make matters even more confusing, there are two predominant ideas of
in-element ordering that often *correlate* with the in-element *byte* ordering
of integer types, but are in fact wholly unrelated! `bitvec` provides these two
main orders as types for you, and if you need a different one, it also provides
the tools you need to make your own.

### Least Significant Bit Comes First

This ordering, named the [`Lsb0`] type, indexes bits within an element by
placing the `0` index at the least significant bit (numeric value `1`) and the
final index at the most significant bit (numeric value `T::min_value()`, for
signed integers on most machines).

For example, this is the ordering used by the [TCP wire format], and by most C
compilers to lay out bit-field struct members on little-endian **byte**-ordered
machines.

### Most Significant Bit Comes First

This ordering, named the [`Msb0`] type, indexes bits within an element by
placing the `0` index at the most significant bit (numeric value `T::min_value()`
for most signed integers) and the final index at the least significant bit
(numeric value `1`).

This is the ordering used by most C compilers to lay out bit-field struct
members on big-endian **byte**-ordered machines.

### Default Ordering

The default ordering is `Lsb0`, as it typically produces shorter object code
than `Msb0` does. If you are implementing a collection, then `Lsb0` is likely
the more performant ordering; if you are implementing a buffer protocol, then
your choice of ordering is dictated by the protocol definition.

# Safety

`BitSlice` is designed to never introduce new memory unsafety that you did not
provide yourself, either before or during the use of this crate. Bugs do, and
have, occured, and you are encouraged to submit any discovered flaw as a defect
report.

The `&BitSlice` reference type uses a private encoding scheme to hold all the
information needed in its stack value. This encoding is **not** part of the
public API of the library, and is not binary-compatible with `&[T]`.
Furthermore, in order to satisfy Rust’s requirements about alias conditions,
`BitSlice` performs type transformations on the `T` parameter to ensure that it
never creates the potential for undefined behavior.

You must never attempt to type-cast a reference to `BitSlice` in any way. You
must not use `mem::transmute` with `BitSlice` anywhere in its type arguments.
You must not use `as`-casting to convert between `*BitSlice` and any other type.
You must not attempt to modify the binary representation of a `&BitSlice`
reference value. These actions will all lead to runtime memory unsafety, are
(hopefully) likely to induce a program crash, and may possibly cause undefined
behavior at compile-time.

Everything in the `BitSlice` public API, even the `unsafe` parts, are guaranteed
to have no more unsafety than their equivalent parts in the standard library.
All `unsafe` APIs will have documentation explicitly detailing what the API
requires you to uphold in order for it to function safely and correctly. All
safe APIs will do so themselves.

# Performance

Like the standard library’s `[T]` slice, `BitSlice` is designed to be very easy
to use safely, while supporting `unsafe` when necessary. Rust has a powerful
optimizing engine, and `BitSlice` will frequently be compiled to have zero
runtime cost. Where it is slower, it will not be significantly slower than a
manual replacement.

As the machine instructions operate on registers rather than bits, your choice
of `T: BitOrder` type parameter can influence your slice’s performance. Using
larger register types means that slices can gallop over completely-filled
interior elements faster, while narrower register types permit more graceful
handling of subslicing and aliased splits.

# Construction

`BitSlice` views of memory can be constructed over borrowed data in a number of
ways. As this is a reference-only type, it can only ever be built by borrowing
an existing memory buffer and taking temporary control of your program’s view of
the region.

## Macro Constructor

`BitSlice` buffers can be constructed at compile-time through the [`bits!`]
macro. This macro accepts a superset of the `vec!` arguments, and creates an
appropriate buffer in your program’s static memory.

```rust
use bitvec::prelude::*;

let static_borrow = bits![0, 1, 0, 0, 1, 0, 0, 1];
let mutable_static: &mut BitSlice<_, _> = bits![mut 0; 8];

assert_ne!(static_borrow, mutable_static);
mutable_static.clone_from_bitslice(static_borrow);
assert_eq!(static_borrow, mutable_static);
```

Note that, despite constructing a `static mut` binding, the `bits![mut …]` call
is not `unsafe`, as the constructed symbol is hidden and only accessible by the
sole `&mut` reference returned by the macro call.

## Borrowing Constructors

The functions [`from_element`], [`from_element_mut`], [`from_slice`], and
[`from_slice_mut`] take references to existing memory, and construct `BitSlice`
references over them. These are the most basic ways to borrow memory and view it
as bits.

```rust
use bitvec::prelude::*;

let data = [0u16; 3];
let local_borrow = BitSlice::<Lsb0, _>::from_slice(&data);

let mut data = [0u8; 5];
let local_mut = BitSlice::<Lsb0, _>::from_slice_mut(&mut data);
```

## Trait Method Constructors

The [`BitView`] trait implements `.view_bits::<O>()` and `.view_bits_mut::<O>()`
methods on elements, arrays not larger than 32 elements, and slices. This trait,
imported in the crate prelude, is *probably* the easiest way for you to borrow
memory.

```rust
use bitvec::prelude::*;

let data = [0u32; 5];
let trait_view = data.view_bits::<Msb0>();

let mut data = 0usize;
let trait_mut = data.view_bits_mut::<Msb0>();
```

## Owned Bit Slices

If you wish to take ownership of a memory region and enforce that it is always
viewed as a `BitSlice` by default, you can use one of the [`BitArray`],
[`BitBox`], or [`BitVec`] types, rather than pairing ordinary buffer types with
the borrowing constructors.

```rust
use bitvec::prelude::*;

let slice = bits![0; 27];
let array = bitarr![LocalBits, u8; 0; 10];
# #[cfg(feature = "alloc")] fn allocs() {
let boxed = bitbox![0; 10];
let vec = bitvec![0; 20];
# } #[cfg(feature = "alloc")] allocs();

// arrays always round up
assert_eq!(array.as_bitslice(), slice[.. 16]);
# #[cfg(feature = "alloc")] fn allocs2() {
# let slice = bits![0; 27];
# let boxed = bitbox![0; 10];
# let vec = bitvec![0; 20];
assert_eq!(boxed.as_bitslice(), slice[.. 10]);
assert_eq!(vec.as_bitslice(), slice[.. 20]);
# } #[cfg(feature = "alloc")] allocs2();
```

[TCP wire format]: https://en.wikipedia.org/wiki/Transmission_Control_Protocol#TCP_segment_structure
[`BitArray`]: ../array/struct.BitArray.html
[`BitBox`]: ../boxed/struct.BitBox.html
[`BitMut<O, T>`]: struct.BitMut.html
[`BitVec`]: ../vec/struct.BitVec.html
[`BitView`]: ../view/trait.BitView.html
[`Lsb0`]: ../order/struct.Lsb0.html
[`Msb0`]: ../order/struct.Msb0.html
[`bits!`]: ../macro.bits.html
[`bitvec::prelude::LocalBits`]: ../order/type.LocalBits.html
[`std::bitset`]: https://en.cppreference.com/w/cpp/utility/bitset
[`std::vector<bool>`]: https://en.cppreference.com/w/cpp/container/vector_bool
**/
#[repr(transparent)]
pub struct BitSlice<O = Lsb0, T = usize>
where
	O: BitOrder,
	T: BitStore,
{
	/// Mark the in-element ordering of bits
	_ord: PhantomData<O>,
	/// Mark the element type of memory
	_typ: PhantomData<[T]>,
	/// Indicate that this is a newtype wrapper over a wholly-untyped slice.
	///
	/// This is necessary in order for the Rust compiler to remove restrictions
	/// on the possible values of references to this slice `&BitSlice` and
	/// `&mut BitSlice`.
	///
	/// Rust has firm requirements that *any* reference that is directly usable
	/// to dereference a real value must conform to its rules about address
	/// liveness, type alignment, and for slices, trustworthy length. It is
	/// undefined behavior for a slice reference *to a dereferencable type* to
	/// violate any of these restrictions.
	///
	/// However, the value of a reference to a zero-sized type has *no* such
	/// restrictions, because that reference can never perform direct memory
	/// access. The compiler will accept any value in a slot typed as `&[()]`,
	/// because the values in it will never be used for a load or store
	/// instruction. If this were `[T]`, then Rust would make the pointer
	/// encoding used to manage values of `&BitSlice` become undefined behavior.
	///
	/// See the `pointer` module for information on the encoding used.
	_mem: [()],
}

/// Constructors are limited to integers only, and not their `Cell`s or atomics.
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore + BitRegister,
{
	/// Constructs a shared `&BitSlice` reference over a shared element.
	///
	/// The [`BitView`] trait, implemented on all `T` elements, provides a
	/// method [`.view_bits::<O>()`] which delegates to this function and may be
	/// more convenient for you to write.
	///
	/// # Parameters
	///
	/// - `elem`: A shared reference to a memory element.
	///
	/// # Returns
	///
	/// A shared `&BitSlice` over the `elem` element.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let elem = 0u8;
	/// let bits = BitSlice::<LocalBits, _>::from_element(&elem);
	/// assert_eq!(bits.len(), 8);
	/// ```
	///
	/// [`BitView`]: ../view/trait.BitView.html
	/// [`.view_bits::<O>()`]: ../view/trait.BitView.html#method.view_bits
	#[inline]
	pub fn from_element(elem: &T) -> &Self {
		unsafe {
			BitPtr::new_unchecked(elem, BitIdx::ZERO, T::Mem::BITS as usize)
		}
		.to_bitslice_ref()
	}

	/// Constructs an exclusive `&mut BitSlice` reference over an element.
	///
	/// The [`BitView`] trait, implemented on all `T` elements, provides a
	/// method [`.view_bits_mut::<O>()`] which delegates to this function and
	/// may be more convenient for you to write.
	///
	/// # Parameters
	///
	/// - `elem`: An exclusive reference to a memory element.
	///
	/// # Returns
	///
	/// An exclusive `&mut BitSlice` over the `elem` element.
	///
	/// Note that the original `elem` reference will be inaccessible for the
	/// duration of the returned slice handle’s lifetime.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut elem = 0u16;
	/// let bits = BitSlice::<Msb0, _>::from_element_mut(&mut elem);
	/// bits.set(15, true);
	/// assert!(bits.get(15).unwrap());
	/// assert_eq!(elem, 1);
	/// ```
	///
	/// [`BitView`]: ../view/trait.BitView.html
	/// [`.view_bits_mut::<O>()`]:
	/// ../view/trait.BitView.html#method.view_bits_mut
	#[inline]
	pub fn from_element_mut(elem: &mut T) -> &mut Self {
		unsafe {
			BitPtr::new_unchecked(elem, BitIdx::ZERO, T::Mem::BITS as usize)
		}
		.to_bitslice_mut()
	}

	/// Constructs a shared `&BitSlice` reference over a shared element slice.
	///
	/// The [`BitView`] trait, implemented on all `[T]` slices, provides a
	/// method [`.view_bits::<O>()`] that is equivalent to this function and may
	/// be more convenient for you to write.
	///
	/// # Parameters
	///
	/// - `slice`: A shared reference over a sequence of memory elements.
	///
	/// # Returns
	///
	/// If `slice` does not have fewer than [`MAX_ELTS`] elements, this returns
	/// `None`. Otherwise, it returns a shared `&BitSlice` over the `slice`
	/// elements.
	///
	/// # Conditions
	///
	/// The produced `&BitSlice` handle always begins at the zeroth bit.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let slice = &[0u8, 1];
	/// let bits = BitSlice::<Msb0, _>::from_slice(slice).unwrap();
	/// assert!(bits[15]);
	/// ```
	///
	/// An example showing this function failing would require a slice exceeding
	/// `!0usize >> 3` bytes in size, which is infeasible to produce.
	///
	/// [`BitView`]: ../view/trait.BitView.html
	/// [`MAX_ELTS`]: #associatedconstant.MAX_ELTS
	/// [`.view_bits::<O>()`]: ../view/trait.BitView.html#method.view_bits
	#[inline]
	pub fn from_slice(slice: &[T]) -> Option<&Self> {
		let elts = slice.len();
		//  Starting at the zeroth bit makes this counter an exclusive cap, not
		//  an inclusive cap.
		if elts >= Self::MAX_ELTS {
			return None;
		}
		Some(unsafe { Self::from_slice_unchecked(slice) })
	}

	/// Converts a slice reference into a `BitSlice` reference without checking
	/// that its size can be safely used.
	///
	/// # Safety
	///
	/// If the `slice` length is too long, then it will be capped at
	/// [`MAX_BITS`]. You are responsible for ensuring that the input slice is
	/// not unduly truncated.
	///
	/// Prefer [`from_slice`].
	///
	/// [`MAX_BITS`]: #associatedconstant.MAX_BITS
	/// [`from_slice`]: #method.from_slice
	#[inline]
	pub unsafe fn from_slice_unchecked(slice: &[T]) -> &Self {
		//  This branch could be removed by lowering the element ceiling by one,
		//  but `from_slice` should not be in any tight loops, so it’s fine.
		let bits = cmp::min(slice.len() * T::Mem::BITS as usize, Self::MAX_BITS);
		BitPtr::new_unchecked(slice.as_ptr(), BitIdx::ZERO, bits)
			.to_bitslice_ref()
	}

	/// Constructs an exclusive `&mut BitSlice` reference over a slice.
	///
	/// The [`BitView`] trait, implemented on all `[T]` slices, provides a
	/// method [`.view_bits_mut::<O>()`] that is equivalent to this function and
	/// may be more convenient for you to write.
	///
	/// # Parameters
	///
	/// - `slice`: An exclusive reference over a sequence of memory elements.
	///
	/// # Returns
	///
	/// An exclusive `&mut BitSlice` over the `slice` elements.
	///
	/// Note that the original `slice` reference will be inaccessible for the
	/// duration of the returned slice handle’s lifetime.
	///
	/// # Panics
	///
	/// This panics if `slice` does not have fewer than [`MAX_ELTS`] elements.
	///
	/// [`MAX_ELTS`]: #associatedconstant.MAX_ELTS
	///
	/// # Conditions
	///
	/// The produced `&mut BitSlice` handle always begins at the zeroth bit of
	/// the zeroth element in `slice`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut slice = [0u8; 2];
	/// let bits = BitSlice::<Lsb0, _>::from_slice_mut(&mut slice).unwrap();
	///
	/// assert!(!bits[0]);
	/// bits.set(0, true);
	/// assert!(bits[0]);
	/// assert_eq!(slice[0], 1);
	/// ```
	///
	/// This example attempts to construct a `&mut BitSlice` handle from a slice
	/// that is too large to index. Either the `vec!` allocation will fail, or
	/// the bit-slice constructor will fail.
	///
	/// ```rust,should_panic
	/// # #[cfg(feature = "alloc")] {
	/// use bitvec::prelude::*;
	///
	/// let mut data = vec![0usize; BitSlice::<LocalBits, usize>::MAX_ELTS];
	/// let bits = BitSlice::<LocalBits, _>::from_slice_mut(&mut data[..]).unwrap();
	/// # }
	/// # #[cfg(not(feature = "alloc"))] panic!("No allocator present");
	/// ```
	///
	/// [`BitView`]: ../view/trait.BitView.html
	/// [`.view_bits_mut::<O>()`]:
	/// ../view/trait.BitView.html#method.view_bits_mut
	#[inline]
	pub fn from_slice_mut(slice: &mut [T]) -> Option<&mut Self> {
		let elts = slice.len();
		if elts >= Self::MAX_ELTS {
			return None;
		}
		Some(unsafe { Self::from_slice_unchecked_mut(slice) })
	}

	/// Converts a slice reference into a `BitSlice` reference without checking
	/// that its size can be safely used.
	///
	/// # Safety
	///
	/// If the `slice` length is too long, then it will be capped at
	/// [`MAX_BITS`]. You are responsible for ensuring that the input slice is
	/// not unduly truncated.
	///
	/// Prefer [`from_slice_mut`].
	///
	/// [`MAX_BITS`]: #associatedconstant.MAX_BITS
	/// [`from_slice_mut`]: #method.from_slice_mut
	#[inline]
	pub unsafe fn from_slice_unchecked_mut(slice: &mut [T]) -> &mut Self {
		let bits = cmp::min(slice.len() * T::Mem::BITS as usize, Self::MAX_BITS);
		BitPtr::new_unchecked(slice.as_ptr(), BitIdx::ZERO, bits)
			.to_bitslice_mut()
	}
}

/// Methods specific to `BitSlice<_, T>`, and not present on `[T]`.
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Produces the empty slice. This is equivalent to `&[]` for ordinary
	/// slices.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits: &BitSlice = BitSlice::empty();
	/// assert!(bits.is_empty());
	/// ```
	#[inline]
	pub fn empty<'a>() -> &'a Self {
		BitPtr::EMPTY.to_bitslice_ref()
	}

	/// Produces the empty mutable slice. This is equivalent to `&mut []` for
	/// ordinary slices.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits: &mut BitSlice = BitSlice::empty_mut();
	/// assert!(bits.is_empty());
	/// ```
	#[inline]
	pub fn empty_mut<'a>() -> &'a mut Self {
		BitPtr::EMPTY.to_bitslice_mut()
	}

	/// Sets the bit value at the given position.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `index`: The bit index to set. It must be in the range `0 ..
	///   self.len()`.
	/// - `value`: The value to be set, `true` for `1` and `false` for `0`.
	///
	/// # Effects
	///
	/// If `index` is valid, then the bit to which it refers is set to `value`.
	///
	/// # Panics
	///
	/// This method panics if `index` is outside the slice domain.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	///
	/// assert!(!bits.get(7).unwrap());
	/// bits.set(7, true);
	/// assert!(bits.get(7).unwrap());
	/// assert_eq!(data, 1);
	/// ```
	///
	/// This example panics when it attempts to set a bit that is out of bounds.
	///
	/// ```rust,should_panic
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![mut 0];
	/// bits.set(1, false);
	/// ```
	#[inline]
	pub fn set(&mut self, index: usize, value: bool) {
		let len = self.len();
		assert!(index < len, "Index out of range: {} >= {}", index, len);
		unsafe {
			self.set_unchecked(index, value);
		}
	}

	/// Sets a bit at an index, without checking boundary conditions.
	///
	/// This is generally not recommended; use with caution! For a safe
	/// alternative, see [`set`].
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `index`: The bit index to set. It must be in the range `0 ..
	///   self.len()`. It will not be checked.
	///
	/// # Effects
	///
	/// The bit at `index` is set to `value`.
	///
	/// # Safety
	///
	/// This method is **not** safe. It performs raw pointer arithmetic to seek
	/// from the start of the slice to the requested index, and set the bit
	/// there. It does not inspect the length of `self`, and it is free to
	/// perform out-of-bounds memory *write* access.
	///
	/// Use this method **only** when you have already performed the bounds
	/// check, and can guarantee that the call occurs with a safely in-bounds
	/// index.
	///
	/// # Examples
	///
	/// This example uses a bit slice of length 2, and demonstrates
	/// out-of-bounds access to the last bit in the element.
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0u8;
	/// let bits = &mut data.view_bits_mut::<Msb0>()[2 .. 4];
	///
	/// assert_eq!(bits.len(), 2);
	/// unsafe {
	///   bits.set_unchecked(5, true);
	/// }
	/// assert_eq!(data, 1);
	/// ```
	///
	/// [`set`]: #method.set
	#[inline]
	pub unsafe fn set_unchecked(&mut self, index: usize, value: bool) {
		self.bitptr().write::<O>(index, value);
	}

	/// Tests if *all* bits in the slice domain are set (logical `∧`).
	///
	/// # Truth Table
	///
	/// ```text
	/// 0 0 => 0
	/// 0 1 => 0
	/// 1 0 => 0
	/// 1 1 => 1
	/// ```
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// Whether all bits in the slice domain are set. The empty slice returns
	/// `true`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 1];
	/// assert!(bits[.. 2].all());
	/// assert!(!bits[2 ..].all());
	/// ```
	#[inline]
	pub fn all(&self) -> bool {
		match self.domain() {
			Domain::Enclave { head, elem, tail } => {
				/* Due to a bug in `rustc`, calling `.value()` on the two
				`BitMask` types, to use `T::Mem | T::Mem == T::Mem`, causes type
				resolution failure and only discovers the
				`for<'a> BitOr<&'a Self>` implementation in the trait bounds
				`T::Mem: BitMemory: IsUnsigned: BitOr<Self> + for<'a> BitOr<&'a Self>`.

				Until this is fixed, routing through the `BitMask`
				implementation suffices. The by-val and by-ref operator traits
				are at the same position in the bounds chain, making this quite
				a strange bug.
				*/
				!O::mask(head, tail) | elem.load_value() == BitMask::ALL
			},
			Domain::Region { head, body, tail } => {
				head.map_or(true, |(head, elem)| {
					!O::mask(head, None) | elem.load_value() == BitMask::ALL
				}) && body.iter().copied().all(|e| e == T::Mem::ALL)
					&& tail.map_or(true, |(elem, tail)| {
						!O::mask(None, tail) | elem.load_value() == BitMask::ALL
					})
			},
		}
	}

	/// Tests if *any* bit in the slice is set (logical `∨`).
	///
	/// # Truth Table
	///
	/// ```text
	/// 0 0 => 0
	/// 0 1 => 1
	/// 1 0 => 1
	/// 1 1 => 1
	/// ```
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// Whether any bit in the slice domain is set. The empty slice returns
	/// `false`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 0];
	/// assert!(bits[.. 2].any());
	/// assert!(!bits[2 ..].any());
	/// ```
	#[inline]
	pub fn any(&self) -> bool {
		match self.domain() {
			Domain::Enclave { head, elem, tail } => {
				O::mask(head, tail) & elem.load_value() != BitMask::ZERO
			},
			Domain::Region { head, body, tail } => {
				head.map_or(false, |(head, elem)| {
					O::mask(head, None) & elem.load_value() != BitMask::ZERO
				}) || body.iter().copied().any(|e| e != T::Mem::ZERO)
					|| tail.map_or(false, |(elem, tail)| {
						O::mask(None, tail) & elem.load_value() != BitMask::ZERO
					})
			},
		}
	}

	/// Tests if *any* bit in the slice is unset (logical `¬∧`).
	///
	/// # Truth Table
	///
	/// ```text
	/// 0 0 => 1
	/// 0 1 => 1
	/// 1 0 => 1
	/// 1 1 => 0
	/// ```
	///
	/// # Parameters
	///
	/// - `&self
	///
	/// # Returns
	///
	/// Whether any bit in the slice domain is unset.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 1];
	/// assert!(!bits[.. 2].not_all());
	/// assert!(bits[2 ..].not_all());
	/// ```
	#[inline]
	pub fn not_all(&self) -> bool {
		!self.all()
	}

	/// Tests if *all* bits in the slice are unset (logical `¬∨`).
	///
	/// # Truth Table
	///
	/// ```text
	/// 0 0 => 1
	/// 0 1 => 0
	/// 1 0 => 0
	/// 1 1 => 0
	/// ```
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// Whether all bits in the slice domain are unset.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![0, 1, 0, 0];
	/// assert!(!bits[.. 2].not_any());
	/// assert!(bits[2 ..].not_any());
	/// ```
	#[inline]
	pub fn not_any(&self) -> bool {
		!self.any()
	}

	/// Tests whether the slice has some, but not all, bits set and some, but
	/// not all, bits unset.
	///
	/// This is `false` if either [`.all`] or [`.not_any`] are `true`.
	///
	/// # Truth Table
	///
	/// ```text
	/// 0 0 => 0
	/// 0 1 => 1
	/// 1 0 => 1
	/// 1 1 => 0
	/// ```
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// Whether the slice domain has mixed content. The empty slice returns
	/// `false`.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0b111_000_10u8;
	/// let bits = bits![1, 1, 0, 0, 1, 0];
	///
	/// assert!(!bits[.. 2].some());
	/// assert!(!bits[2 .. 4].some());
	/// assert!(bits.some());
	/// ```
	///
	/// [`.all`]: #method.all
	/// [`.not_any`]: #method.not_any
	#[inline]
	pub fn some(&self) -> bool {
		self.any() && self.not_all()
	}

	/// Returns the number of ones in the memory region backing `self`.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// The number of high bits in the slice domain.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 0];
	/// assert_eq!(bits[.. 2].count_ones(), 2);
	/// assert_eq!(bits[2 ..].count_ones(), 0);
	/// ```
	#[inline]
	pub fn count_ones(&self) -> usize {
		match self.domain() {
			Domain::Enclave { head, elem, tail } => (O::mask(head, tail)
				& elem.load_value())
			.value()
			.count_ones() as usize,
			Domain::Region { head, body, tail } => {
				head.map_or(0, |(head, elem)| {
					(O::mask(head, None) & elem.load_value())
						.value()
						.count_ones() as usize
				}) + body
					.iter()
					.copied()
					.map(|e| e.count_ones() as usize)
					.sum::<usize>() + tail.map_or(0, |(elem, tail)| {
					(O::mask(None, tail) & elem.load_value())
						.value()
						.count_ones() as usize
				})
			},
		}
	}

	/// Returns the number of zeros in the memory region backing `self`.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// The number of low bits in the slice domain.
	///
	/// # Examples
	///
	/// Basic usage:
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bits = bits![1, 1, 0, 0];
	/// assert_eq!(bits[.. 2].count_zeros(), 0);
	/// assert_eq!(bits[2 ..].count_zeros(), 2);
	/// ```
	#[inline]
	pub fn count_zeros(&self) -> usize {
		match self.domain() {
			Domain::Enclave { head, elem, tail } => (!O::mask(head, tail)
				| elem.load_value())
			.value()
			.count_zeros() as usize,
			Domain::Region { head, body, tail } => {
				head.map_or(0, |(head, elem)| {
					(!O::mask(head, None) | elem.load_value())
						.value()
						.count_zeros() as usize
				}) + body
					.iter()
					.copied()
					.map(|e| e.count_zeros() as usize)
					.sum::<usize>() + tail.map_or(0, |(elem, tail)| {
					(!O::mask(None, tail) | elem.load_value())
						.value()
						.count_zeros() as usize
				})
			},
		}
	}

	/// Sets all bits in the slice to a value.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `value`: The bit value to which all bits in the slice will be set.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut src = 0u8;
	/// let bits = src.view_bits_mut::<Msb0>();
	/// bits[2 .. 6].set_all(true);
	/// assert_eq!(bits.as_slice(), &[0b0011_1100]);
	/// bits[3 .. 5].set_all(false);
	/// assert_eq!(bits.as_slice(), &[0b0010_0100]);
	/// bits[.. 1].set_all(true);
	/// assert_eq!(bits.as_slice(), &[0b1010_0100]);
	/// ```
	#[inline]
	pub fn set_all(&mut self, value: bool) {
		//  Grab the function pointers used to commit bit-masks into memory.
		let setter = <<T::Alias as BitStore>::Access>::get_writers(value);
		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => {
				//  Step three: write the bitmask through the accessor.
				setter(
					//  Step one: attach an `::Access` marker to the reference
					dvl::accessor(elem),
					//  Step two: insert an `::Alias` marker *into the bitmask*
					//  because typechecking is “fun”
					O::mask(head, tail).pipe(dvl::alias_mask::<T>),
				);
			},
			DomainMut::Region { head, body, tail } => {
				if let Some((head, elem)) = head {
					setter(
						dvl::accessor(elem),
						O::mask(head, None).pipe(dvl::alias_mask::<T>),
					);
				}
				//  loop assignment is `memset`’s problem, not ours
				unsafe {
					ptr::write_bytes(
						body.as_mut_ptr(),
						[0, !0][value as usize],
						body.len(),
					);
				}
				if let Some((elem, tail)) = tail {
					setter(
						dvl::accessor(elem),
						O::mask(None, tail).pipe(dvl::alias_mask::<T>),
					);
				}
			},
		}
	}

	/// Applies a function to each bit in the slice.
	///
	/// `BitSlice` cannot implement `IndexMut`, as it cannot manifest `&mut
	/// bool` references, and the [`BitMut`] proxy reference has an unavoidable
	/// overhead. This method bypasses both problems, by applying a function to
	/// each pair of index and value in the slice, without constructing a proxy
	/// reference.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `func`: A function which receives two arguments, `index: usize` and
	///   `value: bool`, and returns a `bool`.
	///
	/// # Effects
	///
	/// For each index in the slice, the result of invoking `func` with the
	/// index number and current bit value is written into the slice.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	/// bits.for_each(|idx, _bit| idx % 3 == 0);
	/// assert_eq!(data, 0b100_100_10);
	/// ```
	#[inline]
	pub fn for_each<F>(&mut self, mut func: F)
	where F: FnMut(usize, bool) -> bool {
		for idx in 0 .. self.len() {
			unsafe {
				let tmp = *self.get_unchecked(idx);
				let new = func(idx, tmp);
				self.set_unchecked(idx, new);
			}
		}
	}

	/// Accesses the total backing storage of the `BitSlice`, as a slice of its
	/// elements.
	///
	/// This method produces a slice over all the memory elements it touches,
	/// using the current storage parameter. This is safe to do, as any events
	/// that would create an aliasing view into the elements covered by the
	/// returned slice will also have caused the slice to use its alias-aware
	/// type.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A view of the entire memory region this slice covers, including the edge
	/// elements.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0x3Cu8;
	/// let bits = &data.view_bits::<LocalBits>()[2 .. 6];
	///
	/// assert!(bits.all());
	/// assert_eq!(bits.len(), 4);
	/// assert_eq!(bits.as_slice(), &[0x3Cu8]);
	/// ```
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_slice(&self) -> &[T] {
		let bitptr = self.bitptr();
		let (base, elts) = (bitptr.pointer().to_const(), bitptr.elements());
		unsafe { slice::from_raw_parts(base, elts) }
	}

	/// Views the wholly-filled elements of the `BitSlice`.
	///
	/// This will not include partially-owned edge elements, as they may be
	/// aliased by other handles. To gain access to all elements that the
	/// `BitSlice` region covers, use one of the following:
	///
	/// - [`.as_slice`] produces a shared slice over all elements, marked
	///   aliased as appropriate.
	/// - [`.domain`] produces a view describing each component of the region,
	///   marking only the contended edges as aliased and the uncontended
	///   interior as unaliased.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A slice of all the wholly-filled elements in the `BitSlice` backing
	/// storage.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = [1u8, 66];
	/// let bits = data.view_bits::<Msb0>();
	///
	/// let accum = bits
	///   .as_raw_slice()
	///   .iter()
	///   .copied()
	///   .map(u8::count_ones)
	///   .sum::<u32>();
	/// assert_eq!(accum, 3);
	/// ```
	///
	/// [`.as_slice`]: #method.as_slice
	/// [`.domain`]: #method.domain
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_raw_slice(&self) -> &[T::Mem] {
		self.domain().region().map_or(&[], |(_, b, _)| b)
	}

	/// Views the wholly-filled elements of the `BitSlice`.
	///
	/// This will not include partially-owned edge elements, as they may be
	/// aliased by other handles. To gain access to all elements that the
	/// `BitSlice` region covers, use one of the following:
	///
	/// - [`.as_aliased_slice`] produces a shared slice over all elements,
	///   marked as aliased to allow for the possibliity of mutation.
	/// - [`.domain_mut`] produces a view describing each component of the
	///   region, marking only the contended edges as aliased and the
	///   uncontended interior as unaliased.
	///
	/// # Parameters
	///
	/// - `&mut self`
	///
	/// # Returns
	///
	/// A mutable slice of all the wholly-filled elements in the `BitSlice`
	/// backing storage.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = [1u8, 64];
	/// let bits = data.view_bits_mut::<Msb0>();
	/// for elt in bits.as_raw_slice_mut() {
	///   *elt |= 2;
	/// }
	/// assert_eq!(&[3, 66], bits.as_slice());
	/// ```
	///
	/// [`.as_aliased_slice`]: #method.as_aliased_slice
	/// [`.domain_mut`]: #method.domain_mut
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn as_raw_slice_mut(&mut self) -> &mut [T::Mem] {
		self.domain_mut().region().map_or(&mut [], |(_, b, _)| b)
	}

	/// Splits the slice into the logical components of its memory domain.
	///
	/// This produces a set of read-only subslices, marking as much as possible
	/// as affirmatively lacking any write-capable view (`T::NoAlias`). The
	/// unaliased view is able to safely perform unsynchronized reads from
	/// memory without causing undefined behavior, as the type system is able to
	/// statically prove that no other write-capable views exist.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A `BitDomain` structure representing the logical components of the
	/// memory region.
	///
	/// # Safety Exception
	///
	/// The following snippet describes a means of constructing a `T::NoAlias`
	/// view into memory that is, in fact, aliased:
	///
	/// ```rust
	/// # #[cfg(feature = "atomic")] {
	/// use bitvec::prelude::*;
	/// use core::sync::atomic::AtomicU8;
	/// type Bs<T> = BitSlice<LocalBits, T>;
	///
	/// let data = [AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)];
	/// let bits: &Bs<AtomicU8> = data.view_bits::<LocalBits>();
	/// let subslice: &Bs<AtomicU8> = &bits[4 .. 20];
	///
	/// let (_, noalias, _): (_, &Bs<u8>, _) =
	///   subslice.bit_domain().region().unwrap();
	/// # }
	/// ```
	///
	/// The `noalias` reference, which has memory type `u8`, assumes that it can
	/// act as an `&u8` reference: unsynchronized loads are permitted, as no
	/// handle exists which is capable of modifying the middle bit of `data`.
	/// This means that LLVM is permitted to issue loads from memory *wherever*
	/// it wants in the block during which `noalias` is live, as all loads are
	/// equivalent.
	///
	/// Use of the `bits` or `subslice` handles, which are still live for the
	/// lifetime of `noalias`, to issue [`.set_aliased`] calls into the middle
	/// element introduce **undefined behavior**. `bitvec` permits safe code to
	/// introduce this undefined behavior solely because it requires deliberate
	/// opt-in – you must start from atomic data; this cannot occur when `data`
	/// is non-atomic – and use of the shared-mutation facility simultaneously
	/// with the unaliasing view.
	///
	/// The [`.set_aliased`] method is speculative, and will be marked as
	/// `unsafe` or removed at any suspicion that its presence in the library
	/// has any costs.
	///
	/// # Examples
	///
	/// This method can be used to accelerate reads from a slice that is marked
	/// as aliased.
	///
	/// ```rust
	/// use bitvec::prelude::*;
	/// type Bs<T> = BitSlice<LocalBits, T>;
	///
	/// let bits = bits![mut LocalBits, u8; 0; 24];
	/// let (a, b): (
	///   &mut Bs<<u8 as BitStore>::Alias>,
	///   &mut Bs<<u8 as BitStore>::Alias>,
	/// ) = bits.split_at_mut(4);
	/// let (partial, full, _): (
	///   &Bs<<u8 as BitStore>::Alias>,
	///   &Bs<<u8 as BitStore>::Mem>,
	///   _,
	/// ) = b.bit_domain().region().unwrap();
	/// read_from(partial); // uses alias-aware reads
	/// read_from(full); // uses ordinary reads
	/// # fn read_from<T: BitStore>(_: &BitSlice<LocalBits, T>) {}
	/// ```
	///
	/// [`.set_aliased`]: #method.set_aliased
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn bit_domain(&self) -> BitDomain<O, T> {
		BitDomain::new(self)
	}

	/// Splits the slice into the logical components of its memory domain.
	///
	/// This produces a set of mutable subslices, marking as much as possible as
	/// affirmatively lacking any other view (`T::Mem`). The bare view is able
	/// to safely perform unsynchronized reads from and writes to memory without
	/// causing undefined behavior, as the type system is able to statically
	/// prove that no other views exist.
	///
	/// # Why This Is More Sound Than `.bit_domain`
	///
	/// The `&mut` exclusion rule makes it impossible to construct two
	/// references over the same memory where one of them is marked `&mut`. This
	/// makes it impossible to hold a live reference to memory *separately* from
	/// any references produced from this method. For the duration of all
	/// references produced by this method, all ancestor references used to
	/// reach this method call are either suspended or dead, and the compiler
	/// will not allow you to use them.
	///
	/// As such, this method cannot introduce undefined behavior where a
	/// reference incorrectly believes that the referent memory region is
	/// immutable.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn bit_domain_mut(&mut self) -> BitDomainMut<O, T> {
		BitDomainMut::new(self)
	}

	/// Splits the slice into immutable references to its underlying memory
	/// components.
	///
	/// Unlike [`.bit_domain`] and [`.bit_domain_mut`], this does not return
	/// smaller `BitSlice` handles but rather appropriately-marked references to
	/// the underlying memory elements.
	///
	/// The aliased references allow mutation of these elements. You are
	/// required to not use mutating methods on these references *at all*. This
	/// function is not marked `unsafe`, but this is a contract you must uphold.
	/// Use [`.domain_mut`] to modify the underlying elements.
	///
	/// > It is not currently possible to forbid mutation through these
	/// > references. This may change in the future.
	///
	/// # Safety Exception
	///
	/// As with [`.bit_domain`], this produces unsynchronized immutable
	/// references over the fully-populated interior elements. If this view is
	/// constructed from a `BitSlice` handle over atomic memory, then it will
	/// remove the atomic access behavior for the interior elements. This *by
	/// itself* is safe, as long as no contemporaneous atomic writes to that
	/// memory can occur. You must not retain and use an atomic reference to the
	/// memory region marked as `NoAlias` for the duration of this view’s
	/// existence.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A read-only descriptor of the memory elements backing `*self`.
	///
	/// [`.bit_domain`]: #method.bit_domain
	/// [`.bit_domain_mut`]: #method.bit_domain_mut
	/// [`.domain_mut`]: #method.domain_mut
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn domain(&self) -> Domain<T> {
		Domain::new(self)
	}

	/// Splits the slice into mutable references to its underlying memory
	/// elements.
	///
	/// Like [`.domain`], this returns appropriately-marked references to the
	/// underlying memory elements. These references are all writable.
	///
	/// The aliased edge references permit modifying memory beyond their bit
	/// marker. You are required to only mutate the region of these edge
	/// elements that you currently govern. This function is not marked
	/// `unsafe`, but this is a contract you must uphold.
	///
	/// > It is not currently possible to forbid out-of-bounds mutation through
	/// > these references. This may change in the future.
	///
	/// # Parameters
	///
	/// - `&mut self`
	///
	/// # Returns
	///
	/// A descriptor of the memory elements underneath `*self`, permitting
	/// mutation.
	///
	/// [`.domain`]: #method.domain
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn domain_mut(&mut self) -> DomainMut<T> {
		DomainMut::new(self)
	}

	/// Splits a slice at some mid-point, without checking boundary conditions.
	///
	/// This is generally not recommended; use with caution! For a safe
	/// alternative, see [`split_at`].
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `mid`: The index at which to split the slice. This must be in the
	///   range `0 .. self.len()`.
	///
	/// # Returns
	///
	/// - `.0`: `&self[.. mid]`
	/// - `.1`: `&self[mid ..]`
	///
	/// # Safety
	///
	/// This function is **not** safe. It performs raw pointer arithmetic to
	/// construct two new references. If `mid` is out of bounds, then the first
	/// slice will be too large, and the second will be *catastrophically*
	/// incorrect. As both are references to invalid memory, they are undefined
	/// to *construct*, and may not ever be used.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let data = 0x0180u16;
	/// let bits = data.view_bits::<Msb0>();
	///
	/// let (one, two) = unsafe { bits.split_at_unchecked(8) };
	/// assert!(one[7]);
	/// assert!(two[0]);
	/// ```
	///
	/// [`split_at`]: #method.split_at
	#[inline]
	pub unsafe fn split_at_unchecked(&self, mid: usize) -> (&Self, &Self) {
		(self.get_unchecked(.. mid), self.get_unchecked(mid ..))
	}

	/// Splits a mutable slice at some mid-point, without checking boundary
	/// conditions.
	///
	/// This is generally not recommended; use with caution! For a safe
	/// alternative, see [`split_at_mut`].
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `mid`: The index at which to split the slice. This must be in the
	///   range `0 .. self.len()`.
	///
	/// # Returns
	///
	/// - `.0`: `&mut self[.. mid]`
	/// - `.1`: `&mut self[mid ..]`
	///
	/// # Safety
	///
	/// This function is **not** safe. It performs raw pointer arithmetic to
	/// construct two new references. If `mid` is out of bounds, then the first
	/// slice will be too large, and the second will be *catastrophically*
	/// incorrect. As both are references to invalid memory, they are undefined
	/// to *construct*, and may not ever be used.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0u16;
	/// let bits = data.view_bits_mut::<Msb0>();
	///
	/// let (one, two) = unsafe { bits.split_at_unchecked_mut(8) };
	/// one.set(7, true);
	/// two.set(0, true);
	/// assert_eq!(data, 0x0180u16);
	/// ```
	///
	/// [`split_at_mut`]: #method.split_at_mut
	#[inline]
	#[allow(clippy::type_complexity)]
	pub unsafe fn split_at_unchecked_mut(
		&mut self,
		mid: usize,
	) -> (&mut BitSlice<O, T::Alias>, &mut BitSlice<O, T::Alias>)
	{
		let bp = self.alias_mut().bitptr();
		(
			bp.to_bitslice_mut().get_unchecked_mut(.. mid),
			bp.to_bitslice_mut().get_unchecked_mut(mid ..),
		)
	}

	/// Splits a mutable slice at some mid-point, without checking boundary
	/// conditions or adding an alias marker.
	///
	/// This method has the same behavior as [`split_at_unchecked_mut`], except
	/// that it does not apply an aliasing marker to the partitioned subslices.
	///
	/// # Safety
	///
	/// See [`split_at_unchecked_mut`] for safety requirements.
	///
	/// Additionally, this is only safe when `T` is alias-safe.
	///
	/// [`split_at_unchecked_mut`]: #method.split_at_unchecked_mut
	#[inline]
	pub(crate) unsafe fn split_at_unchecked_mut_noalias(
		&mut self,
		mid: usize,
	) -> (&mut Self, &mut Self)
	{
		//  Split the slice at the requested midpoint, adding an alias layer
		let (head, tail) = self.split_at_unchecked_mut(mid);
		//  Remove the new alias layer.
		(Self::unalias_mut(head), Self::unalias_mut(tail))
	}

	/// Swaps the bits at two indices without checking boundary conditions.
	///
	/// This is generally not recommended; use with caution! For a safe
	/// alternative, see [`swap`].
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `a`: One index to swap.
	/// - `b`: The other index to swap.
	///
	/// # Effects
	///
	/// The bit at index `a` is written into index `b`, and the bit at index `b`
	/// is written into `a`.
	///
	/// # Safety
	///
	/// Both `a` and `b` must be less than `self.len()`. Indices greater than
	/// the length will cause out-of-bounds memory access, which can lead to
	/// memory unsafety and a program crash.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 8u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	///
	/// unsafe { bits.swap_unchecked(0, 4); }
	///
	/// assert_eq!(data, 128);
	/// ```
	///
	/// [`swap`]: #method.swap
	#[inline]
	pub unsafe fn swap_unchecked(&mut self, a: usize, b: usize) {
		let bit_a = *self.get_unchecked(a);
		let bit_b = *self.get_unchecked(b);
		self.set_unchecked(a, bit_b);
		self.set_unchecked(b, bit_a);
	}

	/// Copies a bit from one index to another without checking boundary
	/// conditions.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `from`: The index whose bit is to be copied
	/// - `to`: The index into which the copied bit is written.
	///
	/// # Effects
	///
	/// The bit at `from` is written into `to`.
	///
	/// # Safety
	///
	/// Both `from` and `to` must be less than `self.len()`, in order for
	/// `self` to legally read from and write to them, respectively.
	///
	/// If `self` had been split from a larger slice, reading from `from` or
	/// writing to `to` may not *necessarily* cause a memory-safety violation in
	/// the Rust model, due to the aliasing system `bitvec` employs. However,
	/// writing outside the bounds of a slice reference is *always* a logical
	/// error, as it causes changes observable by another reference handle.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 1u8;
	/// let bits = data.view_bits_mut::<Lsb0>();
	///
	/// unsafe { bits.copy_unchecked(0, 2) };
	///
	/// assert_eq!(data, 5);
	/// ```
	#[inline]
	pub unsafe fn copy_unchecked(&mut self, from: usize, to: usize) {
		let tmp = *self.get_unchecked(from);
		self.set_unchecked(to, tmp);
	}

	/// Copies bits from one part of the slice to another part of itself.
	///
	/// `src` is the range within `self` to copy from. `dest` is the starting
	/// index of the range within `self` to copy to, which will have the same
	/// length as `src`. The two ranges may overlap. The ends of the two ranges
	/// must be less than or equal to `self.len()`.
	///
	/// # Effects
	///
	/// `self[src]` is copied to `self[dest .. dest + src.end() - src.start()]`.
	///
	/// # Panics
	///
	/// This function will panic if either range exceeds the end of the slice,
	/// or if the end of `src` is before the start.
	///
	/// # Safety
	///
	/// Both the `src` range and the target range `dest .. dest + src.len()`
	/// must not exceed the `self.len()` slice range.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let mut data = 0x07u8;
	/// let bits = data.view_bits_mut::<Msb0>();
	///
	/// unsafe { bits.copy_within_unchecked(5 .., 0); }
	///
	/// assert_eq!(data, 0xE7);
	/// ```
	#[inline]
	pub unsafe fn copy_within_unchecked<R>(&mut self, src: R, dest: usize)
	where R: RangeBounds<usize> {
		let len = self.len();
		let rev = src.contains(&dest);
		let source = dvl::normalize_range(src, len);
		let iter = source.zip(dest .. len);
		if rev {
			for (from, to) in iter.rev() {
				self.copy_unchecked(from, to);
			}
		}
		else {
			for (from, to) in iter {
				self.copy_unchecked(from, to);
			}
		}
	}

	/// Produces the absolute offset in bits between two slice heads.
	///
	/// While this method is sound for any two arbitrary bit slices, the answer
	/// it produces is meaningful *only* when one argument is a strict subslice
	/// of the other. If the two slices are created from different buffers
	/// entirely, a comparison is undefined; if the two slices are disjoint
	/// regions of the same buffer, then the semantically correct distance is
	/// between the tail of the lower and the head of the upper, which this
	/// does not measure.
	///
	/// # Visual Description
	///
	/// Consider the following sequence of bits:
	///
	/// ```text
	/// [ 0 1 2 3 4 5 6 7 8 9 a b ]
	///   |       ^^^^^^^       |
	///   ^^^^^^^^^^^^^^^^^^^^^^^
	/// ```
	///
	/// It does not matter whether there are bits between the tail of the
	/// smaller and the larger slices. The offset is computed from the bit
	/// distance between the two heads.
	///
	/// # Behavior
	///
	/// This function computes the *semantic* distance between the heads, rather
	/// than the *electrical. It does not take into account the `BitOrder`
	/// implementation of the slice. See the [`::electrical_distance`] method
	/// for that comparison.
	///
	/// # Safety and Soundness
	///
	/// One of `self` or `other` must contain the other for this comparison to
	/// be meaningful.
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `other`: Another bit slice. This must be either a strict subregion or
	///   a strict superregion of `self`.
	///
	/// # Returns
	///
	/// The distance in (semantic) bits betwen the heads of each region. The
	/// value is positive when `other` is higher in the address space than
	/// `self`, and negative when `other` is lower in the address space than
	/// `self`.
	///
	/// [`::electrical_distance]`: #method.electrical_comparison
	pub fn offset_from(&self, other: &Self) -> isize {
		let (elts, bits) = unsafe { self.bitptr().ptr_diff(other.bitptr()) };
		elts.saturating_mul(T::Mem::BITS as isize)
			.saturating_add(bits as isize)
	}

	/// Computes the electrical distance between the heads of two slices.
	///
	/// This method uses the slices’ `BitOrder` implementation to compute the
	/// bit position of their heads, then computes the shift distance, in bits,
	/// between them.
	///
	/// This computation presumes that the bits are counted in the same
	/// direction as are bytes in the abstract memory map.
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `other`: Another bit slice. This must be either a strict subregion or
	///   a strict superregion of `self`.
	///
	/// # Returns
	///
	/// The electrical bit distance between the heads of `self` and `other`.
	pub fn electrical_distance(&self, other: &Self) -> isize {
		let this = self.bitptr();
		let that = other.bitptr();
		let (elts, bits) = unsafe {
			let this = BitPtr::new_unchecked(
				this.pointer(),
				BitIdx::new_unchecked(this.head().position::<O>().value()),
				1,
			);
			let that = BitPtr::new_unchecked(
				that.pointer(),
				BitIdx::new_unchecked(that.head().position::<O>().value()),
				1,
			);
			this.ptr_diff(that)
		};
		elts.saturating_mul(T::Mem::BITS as isize)
			.saturating_add(bits as isize)
	}

	/// Marks an immutable slice as referring to aliased memory region.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub(crate) fn alias(&self) -> &BitSlice<O, T::Alias> {
		unsafe { &*(self.as_ptr() as *const BitSlice<O, T::Alias>) }
	}

	/// Marks a mutable slice as describing an aliased memory region.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub(crate) fn alias_mut(&mut self) -> &mut BitSlice<O, T::Alias> {
		unsafe { &mut *(self as *mut Self as *mut BitSlice<O, T::Alias>) }
	}

	/// Removes the aliasing marker from a mutable slice handle.
	///
	/// # Safety
	///
	/// This must only be used when the slice is either known to be unaliased,
	/// or this call is combined with an operation that adds an aliasing marker
	/// and the total number of aliasing markers must remain unchanged.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub(crate) unsafe fn unalias_mut(
		this: &mut BitSlice<O, T::Alias>,
	) -> &mut Self {
		&mut *(this as *mut BitSlice<O, T::Alias> as *mut Self)
	}

	/// Type-cast the slice reference to its pointer structure.
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub(crate) fn bitptr(&self) -> BitPtr<T> {
		BitPtr::from_bitslice_ptr(self.as_ptr())
	}

	/// Constructs a `BitSlice` over aliased memory.
	///
	/// This is restricted so that it can only be used within the crate.
	/// Construction of a `BitSlice` over externally-aliased memory is unsound.
	#[cfg(not(tarpaulin_include))]
	pub(crate) unsafe fn from_aliased_slice_unchecked(
		slice: &[T::Alias],
	) -> &BitSlice<O, T::Alias> {
		BitPtr::new_unchecked(
			slice.as_ptr(),
			BitIdx::ZERO,
			slice.len() * T::Mem::BITS as usize,
		)
		.to_bitslice_ref()
	}
}

/// Methods available only when `T` allows shared mutability.
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore + Radium<Item = <T as BitStore>::Mem>,
{
	/// Splits a mutable slice at some mid-point.
	///
	/// This method has the same behavior as [`split_at_mut`], except that it
	/// does not apply an aliasing marker to the partitioned subslices.
	///
	/// # Safety
	///
	/// Because this method is defined only on `BitSlice`s whose `T` type is
	/// alias-safe, the subslices do not need to be additionally marked.
	///
	/// [`split_at_mut`]: #method.split_at_mut
	#[inline]
	//  `.split_at_mut` is already tested, and `::unalias_mut` is a noöp.
	#[cfg(not(tarpaulin_include))]
	pub fn split_at_aliased_mut(
		&mut self,
		mid: usize,
	) -> (&mut Self, &mut Self)
	{
		let (head, tail) = self.split_at_mut(mid);
		unsafe { (Self::unalias_mut(head), Self::unalias_mut(tail)) }
	}
}

/// Miscellaneous information.
impl<O, T> BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// The inclusive maximum length of a `BitSlice<_, T>`.
	///
	/// As `BitSlice` is zero-indexed, the largest possible index is one less
	/// than this value.
	///
	/// |CPU word width|         Value         |
	/// |-------------:|----------------------:|
	/// |32 bits       |     `0x1fff_ffff`     |
	/// |64 bits       |`0x1fff_ffff_ffff_ffff`|
	pub const MAX_BITS: usize = BitPtr::<T>::REGION_MAX_BITS;
	/// The inclusive maximum length that a slice `[T]` can be for
	/// `BitSlice<_, T>` to cover it.
	///
	/// A `BitSlice<_, T>` that begins in the interior of an element and
	/// contains the maximum number of bits will extend one element past the
	/// cutoff that would occur if the slice began at the zeroth bit. Such a
	/// slice must be manually constructed, but will not otherwise fail.
	///
	/// |Type Bits|Max Elements (32-bit)| Max Elements (64-bit) |
	/// |--------:|--------------------:|----------------------:|
	/// |        8|    `0x0400_0001`    |`0x0400_0000_0000_0001`|
	/// |       16|    `0x0200_0001`    |`0x0200_0000_0000_0001`|
	/// |       32|    `0x0100_0001`    |`0x0100_0000_0000_0001`|
	/// |       64|    `0x0080_0001`    |`0x0080_0000_0000_0001`|
	pub const MAX_ELTS: usize = BitPtr::<T>::REGION_MAX_ELTS;
}

/** Constructs a `&BitSlice` reference from its component data.

This is logically equivalent to [`slice::from_raw_parts`] for `[T]`.

# Lifetimes

- `'a`: The lifetime of the returned bitslice handle. This must be no longer
  than the duration of the referent region, as it is illegal for references to
  dangle.

# Type Parameters

- `O`: The ordering of bits within elements `T`.
- `T`: The type of each memory element in the backing storage region.

# Parameters

- `addr`: The base address of the memory region that the `BitSlice` covers.
- `head`: The index of the first live bit in `*addr`, at which the `BitSlice`
  begins. This is required to be in the range `0 .. T::Mem::BITS`.
- `bits`: The number of live bits, beginning at `head` in `*addr`, that the
  `BitSlice` contains. This must be no greater than `BitSlice::MAX_BITS`.

# Returns

If the input parameters are valid, this returns `Some` shared reference to a
`BitSlice`. The failure conditions causing this to return `None` are:

- `head` is not less than [`T::Mem::BITS`]
- `bits` is greater than [`BitSlice::<O, T>::MAX_BITS`]
- `addr` is not adequately aligned to `T`
- `addr` is so high in the memory space that the region wraps to the base of the
  memory space

# Safety

The memory region described by the returned `BitSlice` must be validly allocated
within the caller’s memory management system. It must also not be modified for
the duration of the lifetime `'a`, unless the `T` type parameter permits safe
shared mutation.

[`BitSlice::<O, T>::MAX_BITS`]: struct.BitSlice.html#associatedconstant.MAX_BITS
[`T::Mem::BITS`]: ../mem/trait.BitMemory.html#associatedconstant.BITS
[`slice::from_raw_parts`]: https://doc.rust-lang.org/core/slice/fn.from_raw_parts.html
**/
#[inline]
pub unsafe fn bits_from_raw_parts<'a, O, T>(
	addr: *const T,
	head: u8,
	bits: usize,
) -> Option<&'a BitSlice<O, T>>
where
	O: BitOrder,
	T: 'a + BitStore + BitMemory,
{
	let head = crate::index::BitIdx::new(head)?;
	BitPtr::new(addr, head, bits).map(BitPtr::to_bitslice_ref)
}

/** Constructs a `&mut BitSlice` reference from its component data.

This is logically equivalent to [`slice::from_raw_parts_mut`] for `[T]`.

# Lifetimes

- `'a`: The lifetime of the returned bitslice handle. This must be no longer
  than the duration of the referent region, as it is illegal for references to
  dangle.

# Type Parameters

- `O`: The ordering of bits within elements `T`.
- `T`: The type of each memory element in the backing storage region.

# Parameters

- `addr`: The base address of the memory region that the `BitSlice` covers.
- `head`: The index of the first live bit in `*addr`, at which the `BitSlice`
  begins. This is required to be in the range `0 .. T::Mem::BITS`.
- `bits`: The number of live bits, beginning at `head` in `*addr`, that the
  `BitSlice` contains. This must be no greater than `BitSlice::MAX_BITS`.

# Returns

If the input parameters are valid, this returns `Some` shared reference to a
`BitSlice`. The failure conditions causing this to return `None` are:

- `head` is not less than [`T::Mem::BITS`]
- `bits` is greater than [`BitSlice::<O, T>::MAX_BITS`]
- `addr` is not adequately aligned to `T`
- `addr` is so high in the memory space that the region wraps to the base of the
  memory space

# Safety

The memory region described by the returned `BitSlice` must be validly allocated
within the caller’s memory management system. It must also not be reachable for
the lifetime `'a` by any path other than references derived from the return
value.

[`BitSlice::<O, T>::MAX_BITS`]: struct.BitSlice.html#associatedconstant.MAX_BITS
[`T::Mem::BITS`]: ../mem/trait.BitMemory.html#associatedconstant.BITS
[`slice::from_raw_parts_mut`]: https://doc.rust-lang.org/core/slice/fn.from_raw_parts_mut.html
**/
#[inline]
pub unsafe fn bits_from_raw_parts_mut<'a, O, T>(
	addr: *mut T,
	head: u8,
	bits: usize,
) -> Option<&'a mut BitSlice<O, T>>
where
	O: BitOrder,
	T: 'a + BitStore + BitMemory,
{
	let head = crate::index::BitIdx::new(head)?;
	BitPtr::new(addr, head, bits).map(BitPtr::to_bitslice_mut)
}

mod api;
mod iter;
mod ops;
mod proxy;
mod traits;

//  Match the `core::slice` API module topology.

pub use self::{
	api::{
		from_mut,
		from_raw_parts,
		from_raw_parts_mut,
		from_ref,
		BitSliceIndex,
	},
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
	proxy::BitMut,
};

#[cfg(test)]
mod tests;
