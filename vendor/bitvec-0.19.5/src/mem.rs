/*! Descriptions of integer types

This module describes the integer types used to hold bare data. This module
governs the way the processor manipulates integer regions of memory, without
concern for interaction with specifics of register or bus behavior.
!*/

use core::mem;

use funty::IsUnsigned;

/** Description of an integer type.

This trait provides information used to describe integer-typed regions of memory
and enables other parts of the crate to adequately describe the memory bus. This
trait has **no** bearing on the processor instructions or registers used to
interact with memory.

This trait cannot be implemented outside this crate.
**/
pub trait BitMemory: IsUnsigned + seal::Sealed {
	/// The bit width of the integer.
	///
	/// `mem::size_of` returns the size in bytes, and bytes are always eight
	/// bits on architectures Rust targets.
	///
	/// Issue #76904 will place this constant on the fundamentals directly, as a
	/// `u32`.
	const BITS: u8 = mem::size_of::<Self>() as u8 * 8;
	/// The number of bits required to store an index in the range `0 .. BITS`.
	const INDX: u8 = Self::BITS.trailing_zeros() as u8;
	/// A mask over all bits that can be used as an index within the element.
	const MASK: u8 = Self::BITS - 1;

	/// The value with only its least significant bit set to `1`.
	const ONE: Self;
	/// The value with all of its bits set to `1`.
	const ALL: Self;
}

macro_rules! memory {
	($($t:ty),+ $(,)?) => { $(
		impl BitMemory for $t {
			const ONE: Self = 1;
			const ALL: Self = !0;
		}
		impl seal::Sealed for $t {}
	)+ };
}

memory!(u8, u16, u32, u64, u128, usize);

/** Computes the number of elements required to store some number of bits.

# Parameters

- `bits`: The number of bits to store in a `[T]` array.

# Returns

The number of elements `T` required to store `bits`.

As this is a const function, when `bits` is a constant expression, this can be
used to compute the size of an array type `[T; elts(bits)]`.
**/
#[doc(hidden)]
pub const fn elts<T>(bits: usize) -> usize {
	let width = mem::size_of::<T>() * 8;
	bits / width + (bits % width != 0) as usize
}

/** Tests that a type is aligned to its size.

This property is not necessarily true for all integers; for instance, `u64` on
32-bit x86 is permitted to be 4-byte-aligned. `bitvec` requires this property to
hold for the pointer representation to correctly function.

# Type Parameters

- `T`: A type whose alignment and size are to be compared

# Returns

`0` if the alignment matches the size; `1` if they differ
**/
#[doc(hidden)]
pub(crate) const fn aligned_to_size<T>() -> usize {
	(mem::align_of::<T>() != mem::size_of::<T>()) as usize
}

/** Tests whether two types have compatible layouts.

# Type Parameters

- `A`
- `B`

# Returns

Zero if `A` and `B` have equal alignments and sizes, non-zero if they do not.

# Uses

This function is designed to be used in the expression
`const CHECK: [(): 0] = [(); cmp_layout::<A, B>()];`. It will cause a compiler
error if the conditions do not hold.
**/
#[doc(hidden)]
pub(crate) const fn cmp_layout<A, B>() -> usize {
	(mem::align_of::<A>() != mem::align_of::<B>()) as usize
		+ (mem::size_of::<A>() != mem::size_of::<B>()) as usize
}

#[doc(hidden)]
mod seal {
	#[doc(hidden)]
	pub trait Sealed {}
}
