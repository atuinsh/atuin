/*! Parallel bitfield access.

This module provides parallel, multiple-bit, access to a `BitSlice`. This
functionality permits the use of `BitSlice` as a library-level implementation of
the bitfield language feature found in C and C++.

The `BitField` trait is not sealed against client implementation, as there is no
useful way to automatically use a `BitOrder` implementation to provide a
universal behavior. As such, the trait has some requirements that the compiler
cannot enforce for client implementations.

# Batch Behavior

The purpose of this trait is to provide access to arbitrary bit regions as if
they were an ordinary memory location. As such, it is important for
implementations of this trait to provide shift/mask register transfer behavior
where possible, for as wide a span as possible in each action. Implementations
of this trait should *not* use bit-by-bit iteration.

# Register Bit Order Preservation

As a default assumption – user orderings *may* violate this, but *should* not –
each element of slice memory used to store part of a value should not reorder
the value bits. Transfer between slice memory and a CPU register should solely
be an ordinary value load or store between memory and the register, and a
shift/mask operation to select the part of the value that is live.

# Endianness

The `_le` and `_be` methods of `BitField` refer to the order in which
`T: BitStore` elements of the slice are assigned significance when containing
fragments of a stored data value. Within any `T` element, the order of its
constituent bytes is *not* governed by the `BitField` trait method.

The provided `BitOrder` implementors `Lsb0` and `Msb0` use the local machine’s
byte ordering. Other cursors *may* implement ordering of bytes within `T`
elements differently, for instance by calling `.to_be_bytes` before store and
`from_be_bytes` after load.
!*/

use crate::{
	access::BitAccess,
	array::BitArray,
	devel as dvl,
	domain::{
		Domain,
		DomainMut,
	},
	index::BitMask,
	mem::BitMemory,
	order::{
		BitOrder,
		Lsb0,
		Msb0,
	},
	slice::BitSlice,
	store::BitStore,
	view::BitView,
};

use core::{
	mem,
	ops::{
		Shl,
		Shr,
	},
	ptr,
};

use tap::pipe::Pipe;

#[cfg(feature = "alloc")]
use crate::{
	boxed::BitBox,
	vec::BitVec,
};

/** Performs C-style bitfield access through a `BitSlice`.

Bit orderings that permit batched access to regions of memory are enabled to
load data from, and store data to, a `BitStore` with faster behavior than the
default bit-by-bit traversal.

This trait transfers data between a `BitSlice` and a local element. The trait
functions always place the live bit region of the slice against the least
significant bit edge of the local element (return value of `load`, argument of
`store`).

Implementations are encouraged to preserve in-memory bit ordering within a
memory element, so that call sites can provide a value pattern that the user can
clearly see matches what they expect for memory ordering. These methods should
only move data between locations, without modifying the data itself.

Methods should be called as `bits[start .. end].load_or_store()`, where the
range subslice selects no mor than the `M::BITS` element width being
transferred.
**/
pub trait BitField {
	/// Loads the bits in the `self` region into a local value.
	///
	/// This can load into any of the unsigned integers which implement
	/// `BitMemory`. Any further transformation must be done by the user.
	///
	/// The default implementation of this function calls [`load_le`] on
	/// little-endian byte-ordered CPUs, and [`load_be`] on big-endian
	/// byte-ordered CPUs.
	///
	/// # Parameters
	///
	/// - `&self`: A read reference to some bits in memory. This slice must be
	///   trimmed to have a width no more than the `M::BITS` width of the type
	///   being loaded. This can be accomplished with range indexing on a larger
	///   slice.
	///
	/// # Returns
	///
	/// A value `M` whose least `self.len()` significant bits are filled with
	/// the bits of `self`.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	///
	/// [`load_be`]: #tymethod.load_be
	/// [`load_le`]: #tymethod.load_le
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	fn load<M>(&self) -> M
	where M: BitMemory {
		#[cfg(target_endian = "little")]
		return self.load_le::<M>();

		#[cfg(target_endian = "big")]
		return self.load_be::<M>();
	}

	/// Stores a sequence of bits from the user into the domain of `self`.
	///
	/// This can store any of the unsigned integers which implement
	/// `BitMemory`. Any other types must first be transformed by the user.
	///
	/// The default implementation of this function calls [`store_le`] on
	/// little-endian byte-ordered CPUs, and [`store_be`] on big-endian
	/// byte-ordered CPUs.
	///
	/// # Parameters
	///
	/// - `&mut self`: A write reference to some bits in memory. This slice must
	///   be trimmed to have a width no more than the `M::BITS` width of the
	///   type being stored. This can be accomplished with range indexing on a
	///   larger slice.
	/// - `value`: A value, whose `self.len()` least significant bits will be
	///   stored into `self`.
	///
	/// # Behavior
	///
	/// The `self.len()` least significant bits of `value` are written into the
	/// domain of `self`.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	///
	/// [`store_be`]: #tymethod.store_be
	/// [`store_le`]: #tymethod.store_le
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	fn store<M>(&mut self, value: M)
	where M: BitMemory {
		#[cfg(target_endian = "little")]
		self.store_le(value);

		#[cfg(target_endian = "big")]
		self.store_be(value);
	}

	/// Loads from `self`, using little-endian element `T` ordering.
	///
	/// This function interprets a multi-element slice as having its least
	/// significant chunk in the low memory address, and its most significant
	/// chunk in the high memory address. Each element `T` is still interpreted
	/// from individual bytes according to the local CPU ordering.
	///
	/// # Parameters
	///
	/// - `&self`: A read reference to some bits in memory. This slice must be
	///   trimmed to have a width no more than the `M::BITS` width of the type
	///   being loaded. This can be accomplished with range indexing on a larger
	///   slice.
	///
	/// # Returns
	///
	/// A value `M` whose least `self.len()` significant bits are filled with
	/// the bits of `self`. If `self` spans multiple elements `T`, then the
	/// lowest-address `T` is interpreted as containing the least significant
	/// bits of the return value `M`, and the highest-address `T` is interpreted
	/// as containing its most significant bits.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	fn load_le<M>(&self) -> M
	where M: BitMemory;

	/// Loads from `self`, using big-endian element `T` ordering.
	///
	/// This function interprets a multi-element slice as having its most
	/// significant chunk in the low memory address, and its least significant
	/// chunk in the high memory address. Each element `T` is still interpreted
	/// from individual bytes according to the local CPU ordering.
	///
	/// # Parameters
	///
	/// - `&self`: A read reference to some bits in memory. This slice must be
	///   trimmed to have a width no more than the `M::BITS` width of the type
	///   being loaded. This can be accomplished with range indexing on a larger
	///   slice.
	///
	/// # Returns
	///
	/// A value `M` whose least `self.len()` significant bits are filled with
	/// the bits of `self`. If `self` spans multiple elements `T`, then the
	/// lowest-address `T` is interpreted as containing the most significant
	/// bits of the return value `M`, and the highest-address `T` is interpreted
	/// as containing its least significant bits.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	fn load_be<M>(&self) -> M
	where M: BitMemory;

	/// Stores into `self`, using little-endian element ordering.
	///
	/// This function interprets a multi-element slice as having its least
	/// significant chunk in the low memory address, and its most significant
	/// chunk in the high memory address. Each element `T` is still interpreted
	/// from individual bytes according to the local CPU ordering.
	///
	/// # Parameters
	///
	/// - `&mut self`: A write reference to some bits in memory. This slice must
	///   be trimmed to have a width no more than the `M::BITS` width of the
	///   type being stored. This can be accomplished with range indexing on a
	///   larger slice.
	/// - `value`: A value, whose `self.len()` least significant bits will be
	///   stored into `self`.
	///
	/// # Behavior
	///
	/// The `self.len()` least significant bits of `value` are written into the
	/// domain of `self`. If `self` spans multiple elements `T`, then the
	/// lowest-address `T` is interpreted as containing the least significant
	/// bits of the `M` return value, and the highest-address `T` is interpreted
	/// as containing its most significant bits.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	fn store_le<M>(&mut self, value: M)
	where M: BitMemory;

	/// Stores into `self`, using big-endian element ordering.
	///
	/// This function interprets a multi-element slice as having its most
	/// significant chunk in the low memory address, and its least significant
	/// chunk in the high memory address. Each element `T` is still interpreted
	/// from individual bytes according to the local CPU ordering.
	///
	/// # Parameters
	///
	/// - `&mut self`: A write reference to some bits in memory. This slice must
	///   be trimmed to have a width no more than the `M::BITS` width of the
	///   type being stored. This can be accomplished with range indexing on a
	///   larger slice.
	/// - `value`: A value, whose `self.len()` least significant bits will be
	///   stored into `self`.
	///
	/// # Behavior
	///
	/// The `self.len()` least significant bits of `value` are written into the
	/// domain of `self`. If `self` spans multiple elements `T`, then the
	/// lowest-address `T` is interpreted as containing the most significant
	/// bits of the `M` return value, and the highest-address `T` is interpreted
	/// as containing its least significant bits.
	///
	/// # Panics
	///
	/// This method is encouraged to panic if `self` is empty, or wider than a
	/// single element `M`.
	fn store_be<M>(&mut self, value: M)
	where M: BitMemory;
}

impl<T> BitField for BitSlice<Lsb0, T>
where T: BitStore
{
	#[inline]
	fn load_le<M>(&self) -> M
	where M: BitMemory {
		let len = self.len();
		check("load", len, M::BITS);

		match self.domain() {
			//  In Lsb0, a `head` index counts distance from LSedge, and a
			//  `tail` index counts element width minus distance from MSedge.
			Domain::Enclave { head, elem, tail } => {
				get::<T, M>(elem, Lsb0::mask(head, tail), head.value())
			},
			Domain::Region { head, body, tail } => {
				let mut accum = M::ZERO;

				/* For multi-`T::Mem` domains, the most significant chunk is
				stored in the highest memory address, the tail. Each successive
				memory address lower has a chunk of decreasing significance,
				until the least significant chunk is stored in the lowest memory
				address, the head.
				*/

				if let Some((elem, tail)) = tail {
					accum = get::<T, M>(elem, Lsb0::mask(None, tail), 0);
				}

				for elem in body.iter().rev().copied() {
					/* Rust does not allow the use of shift instructions of
					exactly a type width to clear a value. This loop only enters
					when `M` is not narrower than `T::Mem`, and the shift is
					only needed when `M` occupies *more than one* `T::Mem` slot.
					When `M` is exactly as wide as `T::Mem`, this loop either
					does not runs (head and tail only), or runs once (single
					element), and thus the shift is unnecessary.

					As a const-expression, this branch folds at compile-time to
					conditionally remove or retain the instruction.
					*/
					if M::BITS > T::Mem::BITS {
						accum <<= T::Mem::BITS;
					}
					accum |= resize::<T::Mem, M>(elem);
				}

				if let Some((head, elem)) = head {
					let shamt = head.value();
					accum <<= T::Mem::BITS - shamt;
					accum |= get::<T, M>(elem, Lsb0::mask(head, None), shamt);
				}

				accum
			},
		}
	}

	#[inline]
	fn load_be<M>(&self) -> M
	where M: BitMemory {
		let len = self.len();
		check("load", len, M::BITS);

		match self.domain() {
			Domain::Enclave { head, elem, tail } => {
				get::<T, M>(elem, Lsb0::mask(head, tail), head.value())
			},
			Domain::Region { head, body, tail } => {
				let mut accum = M::ZERO;

				if let Some((head, elem)) = head {
					accum =
						get::<T, M>(elem, Lsb0::mask(head, None), head.value());
				}

				for elem in body.iter().copied() {
					if M::BITS > T::Mem::BITS {
						accum <<= T::Mem::BITS;
					}
					accum |= resize::<T::Mem, M>(elem);
				}

				if let Some((elem, tail)) = tail {
					accum <<= tail.value();
					accum |= get::<T, M>(elem, Lsb0::mask(None, tail), 0);
				}

				accum
			},
		}
	}

	#[inline]
	fn store_le<M>(&mut self, mut value: M)
	where M: BitMemory {
		let len = self.len();
		check("store", len, M::BITS);

		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => {
				set::<T, M>(elem, value, Lsb0::mask(head, tail), head.value())
			},
			DomainMut::Region { head, body, tail } => {
				if let Some((head, elem)) = head {
					let shamt = head.value();
					set::<T, M>(elem, value, Lsb0::mask(head, None), shamt);
					value >>= T::Mem::BITS - shamt;
				}

				for elem in body {
					*elem = resize(value);
					if M::BITS > T::Mem::BITS {
						value >>= T::Mem::BITS;
					}
				}

				if let Some((elem, tail)) = tail {
					set::<T, M>(elem, value, Lsb0::mask(None, tail), 0);
				}
			},
		}
	}

	#[inline]
	fn store_be<M>(&mut self, mut value: M)
	where M: BitMemory {
		let len = self.len();
		check("store", len, M::BITS);

		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => {
				set::<T, M>(elem, value, Lsb0::mask(head, tail), head.value())
			},
			DomainMut::Region { head, body, tail } => {
				if let Some((elem, tail)) = tail {
					set::<T, M>(elem, value, Lsb0::mask(None, tail), 0);
					value >>= tail.value()
				}

				for elem in body.iter_mut().rev() {
					*elem = resize(value);
					if M::BITS > T::Mem::BITS {
						value >>= T::Mem::BITS;
					}
				}

				if let Some((head, elem)) = head {
					set::<T, M>(
						elem,
						value,
						Lsb0::mask(head, None),
						head.value(),
					);
				}
			},
		}
	}
}

impl<T> BitField for BitSlice<Msb0, T>
where T: BitStore
{
	#[inline]
	fn load_le<M>(&self) -> M
	where M: BitMemory {
		let len = self.len();
		check("load", len, M::BITS);

		match self.domain() {
			Domain::Enclave { head, elem, tail } => get::<T, M>(
				elem,
				Msb0::mask(head, tail),
				T::Mem::BITS - tail.value(),
			),
			Domain::Region { head, body, tail } => {
				let mut accum = M::ZERO;

				if let Some((elem, tail)) = tail {
					accum = get::<T, M>(
						elem,
						Msb0::mask(None, tail),
						T::Mem::BITS - tail.value(),
					);
				}

				for elem in body.iter().rev().copied() {
					if M::BITS > T::Mem::BITS {
						accum <<= T::Mem::BITS;
					}
					accum |= resize::<T::Mem, M>(elem);
				}

				if let Some((head, elem)) = head {
					accum <<= T::Mem::BITS - head.value();
					accum |= get::<T, M>(elem, Msb0::mask(head, None), 0);
				}

				accum
			},
		}
	}

	#[inline]
	fn load_be<M>(&self) -> M
	where M: BitMemory {
		let len = self.len();
		check("load", len, M::BITS);

		match self.domain() {
			Domain::Enclave { head, elem, tail } => get::<T, M>(
				elem,
				Msb0::mask(head, tail),
				T::Mem::BITS - tail.value(),
			),
			Domain::Region { head, body, tail } => {
				let mut accum = M::ZERO;

				if let Some((head, elem)) = head {
					accum = get::<T, M>(elem, Msb0::mask(head, None), 0);
				}

				for elem in body.iter().copied() {
					if M::BITS > T::Mem::BITS {
						accum <<= T::Mem::BITS;
					}
					accum |= resize::<T::Mem, M>(elem);
				}

				if let Some((elem, tail)) = tail {
					let width = tail.value();
					accum <<= width;
					accum |= get::<T, M>(
						elem,
						Msb0::mask(None, tail),
						T::Mem::BITS - width,
					);
				}

				accum
			},
		}
	}

	#[inline]
	fn store_le<M>(&mut self, mut value: M)
	where M: BitMemory {
		let len = self.len();
		check("store", len, M::BITS);

		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => set::<T, M>(
				elem,
				value,
				Msb0::mask(head, tail),
				T::Mem::BITS - tail.value(),
			),
			DomainMut::Region { head, body, tail } => {
				if let Some((head, elem)) = head {
					set::<T, M>(elem, value, Msb0::mask(head, None), 0);
					value >>= T::Mem::BITS - head.value();
				}

				for elem in body.iter_mut() {
					*elem = resize(value);
					if M::BITS > T::Mem::BITS {
						value >>= T::Mem::BITS;
					}
				}

				if let Some((elem, tail)) = tail {
					set::<T, M>(
						elem,
						value,
						Msb0::mask(None, tail),
						T::Mem::BITS - tail.value(),
					);
				}
			},
		}
	}

	#[inline]
	fn store_be<M>(&mut self, mut value: M)
	where M: BitMemory {
		let len = self.len();
		check("store", len, M::BITS);

		match self.domain_mut() {
			DomainMut::Enclave { head, elem, tail } => set::<T, M>(
				elem,
				value,
				Msb0::mask(head, tail),
				T::Mem::BITS - tail.value(),
			),
			DomainMut::Region { head, body, tail } => {
				if let Some((elem, tail)) = tail {
					set::<T, M>(
						elem,
						value,
						Msb0::mask(None, tail),
						T::Mem::BITS - tail.value(),
					);
					value >>= tail.value();
				}

				for elem in body.iter_mut().rev() {
					*elem = resize(value);
					if M::BITS > T::Mem::BITS {
						value >>= T::Mem::BITS;
					}
				}

				if let Some((head, elem)) = head {
					set::<T, M>(elem, value, Msb0::mask(head, None), 0);
				}
			},
		}
	}
}

#[cfg(not(tarpaulin_include))]
impl<O, V> BitField for BitArray<O, V>
where
	O: BitOrder,
	V: BitView,
	BitSlice<O, V::Store>: BitField,
{
	#[inline]
	fn load_le<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_le()
	}

	#[inline]
	fn load_be<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_be()
	}

	#[inline]
	fn store_le<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_le(value)
	}

	#[inline]
	fn store_be<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_be(value)
	}
}

#[cfg(feature = "alloc")]
#[cfg(not(tarpaulin_include))]
impl<O, T> BitField for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitField,
{
	#[inline]
	fn load_le<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_le()
	}

	#[inline]
	fn load_be<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_be()
	}

	#[inline]
	fn store_le<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_le(value)
	}

	#[inline]
	fn store_be<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_be(value)
	}
}

#[cfg(feature = "alloc")]
#[cfg(not(tarpaulin_include))]
impl<O, T> BitField for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
	BitSlice<O, T>: BitField,
{
	#[inline]
	fn load_le<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_le()
	}

	#[inline]
	fn load_be<M>(&self) -> M
	where M: BitMemory {
		self.as_bitslice().load_be()
	}

	#[inline]
	fn store_le<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_le(value)
	}

	#[inline]
	fn store_be<M>(&mut self, value: M)
	where M: BitMemory {
		self.as_mut_bitslice().store_be(value)
	}
}

/// Asserts that a slice length is within a memory element width.
#[inline]
fn check(action: &'static str, len: usize, width: u8) {
	if !(1 ..= width as usize).contains(&len) {
		panic!("Cannot {} {} bits from a {}-bit region", action, width, len);
	}
}

/** Reads a value out of a section of a memory element.

This function is used to extract a portion of an `M` value from a portion of a
`T` value. The `BitField` implementations call it as they assemble a complete
`M`. It performs the following steps:

1. the referent value of the `elem` pointer is copied into local memory,
2. `mask`ed to discard the portions of `*elem` that are not live,
3. shifted to the LSedge of the `T::Mem` temporary,
4. then `resize`d into an `M` value.

This is the exact inverse of `set`.

# Type Parameters

- `T`: The `BitStore` type of a `BitSlice` that is the source of a read event.
- `M`: The local type of the data contained in that `BitSlice`.

# Parameters

- `elem`: An aliased reference to a single element of a `BitSlice` storage. This
  is required to remain aliased, as other write-capable references to the
  location may exist.
- `mask`: A `BitMask` of the live region of the value at `*elem` to be used as
  the contents of the returned value.
- `shamt`: The distance of the least significant bit of the mask region from the
  least significant edge of the `T::Mem` fetched value.

# Returns

`resize((*elem & mask) >> shamt)`
**/
#[inline]
fn get<T, M>(elem: &T, mask: BitMask<T::Mem>, shamt: u8) -> M
where
	T: BitStore,
	M: BitMemory,
{
	elem.load_value()
		.pipe(|val| mask & val)
		.value()
		.pipe(|val| Shr::<u8>::shr(val, shamt))
		.pipe(resize::<T::Mem, M>)
}

/** Writes a value into a section of a memory element.

This function is used to emplace a portion of an `M` value into a portion of a
`T` value. The `BitField` implementations call it as they disassemble a complete
`M`. It performs the following steps:

1. the provided `value` is `resize`d from `M` to `T::Mem`,
2. then shifted from the LSedge of the `T::Mem` temporary by `shamt`,
3. `mask`ed to discard the portions of `value` that are not live,
4. then written into the `mask`ed portion of `*elem`.

This is the exact inverse of `get`.

# Type Parameters

- `T`: The `BitStore` type of a `BitSlice` that is the sink of a write event.
- `M`: The local type of the data being written into that `BitSlice`.

# Parameters

- `elem`: An aliased reference to a single element of a `BitSlice` storage.
- `value`: The value whose least-significant bits will be written into the
  subsection of `*elt` covered by `mask`.
- `mask`: A `BitMask` of the live region of the value at `*elem` to be used as
  a filter on the provided value.
- `shamt`: The distance of the least significant bit of the mask region from the
  least significant edge of the `T::Mem` destination value.

# Effects

`*elem &= !mask; *elem |= (resize(value) << shamt) & mask;`
**/
#[inline]
fn set<T, M>(elem: &T::Alias, value: M, mask: BitMask<T::Mem>, shamt: u8)
where
	T: BitStore,
	M: BitMemory,
{
	//  Convert the aliasing reference into its accessing type.
	let elem = dvl::accessor(elem);
	//  Mark the mask as aliased, to fit into the accessor reference.
	let mask = dvl::alias_mask::<T>(mask);
	//  Modify `value` to fit the accessor reference, by:
	let value = value
		//  resizing from `M` to `T::Mem`,
		.pipe(resize::<M, T::Mem>)
		//  marking it as `T::Alias::Mem`,
		.pipe(dvl::alias_mem::<T>)
		//  and shifting it left by `shamt` to be in the mask region,
		.pipe(|val| Shl::<u8>::shl(val, shamt))
		//  then masking it.
		.pipe(|val| mask & val);

	elem.clear_bits(mask);
	elem.set_bits(value);
}

/** Resizes a value from one register width to another

This zero-extends or truncates its source value in order to fit in the target
type.

# Type Parameters

- `T`: The initial register type of the value to resize.
- `U`: The final register type of the resized value.

# Parameters

- `value`: Any register value

# Returns

`value`, either zero-extended if `U` is wider than `T` or truncated if `U` is
narrower than `T`.
**/
#[inline]
fn resize<T, U>(value: T) -> U
where
	T: BitMemory,
	U: BitMemory,
{
	let mut out = U::ZERO;
	let size_t = mem::size_of::<T>();
	let size_u = mem::size_of::<U>();

	unsafe {
		resize_inner::<T, U>(&value, &mut out, size_t, size_u);
	}

	out
}

/// Performs little-endian byte-order register resizing.
#[inline(always)]
#[cfg(target_endian = "little")]
#[cfg(not(tarpaulin_include))]
unsafe fn resize_inner<T, U>(
	src: &T,
	dst: &mut U,
	size_t: usize,
	size_u: usize,
)
{
	//  In LE, the least significant byte is the base address, so resizing is
	//  just a memcpy into a zeroed slot, taking only the smaller width.
	ptr::copy_nonoverlapping(
		src as *const T as *const u8,
		dst as *mut U as *mut u8,
		core::cmp::min(size_t, size_u),
	);
}

/// Performs big-endian byte-order register resizing.
#[inline(always)]
#[cfg(target_endian = "big")]
#[cfg(not(tarpaulin_include))]
unsafe fn resize_inner<T, U>(
	src: &T,
	dst: &mut U,
	size_t: usize,
	size_u: usize,
)
{
	let src = src as *const T as *const u8;
	let dst = dst as *mut U as *mut u8;

	//  In BE, shrinking a value requires moving the source base pointer up,
	if size_t > size_u {
		ptr::copy_nonoverlapping(src.add(size_t - size_u), dst, size_u);
	}
	//  While expanding a value requires moving the destination base pointer up.
	else {
		ptr::copy_nonoverlapping(src, dst.add(size_u - size_t), size_t);
	}
}

#[cfg(not(any(target_endian = "big", target_endian = "little")))]
compile_fail!(concat!(
	"This architecture is currently not supported. File an issue at ",
	env!(CARGO_PKG_REPOSITORY)
));

#[cfg(feature = "std")]
mod io;

#[cfg(test)]
mod tests;

// These tests are purely mathematical, and do not need to run more than once.
#[cfg(all(test, feature = "std", not(miri), not(tarpaulin)))]
mod permutation_tests;
