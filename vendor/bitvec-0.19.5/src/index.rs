/*! Typed metadata of registers.

This module provides types which guarantee certain properties about working with
individual bits of registers.

The main advantage of the types in this module is that they provide
type-dependent range constrictions for index values, making it impossible to
have an index out of bounds for a register, and creating a sequence of type
transformations that give assurance about the continued validity of each value
in its surrounding context.

By eliminating public constructors from arbitrary integers, `bitvec` can
guarantee that only it can produce seed values, and only trusted functions can
transform their numeric values or types, until the program reaches the property
it requires. This chain of assurance means that operations that interact with
memory can be confident in the correctness of their actions and effects.

# Type Sequence

The library produces `BitIdx` values from region computation. These types cannot
be publicly constructed, and are only ever the result of pointer analysis. As
such, they rely on correctness of the memory regions provided to library entry
points, and those entry points can leverage the Rust type system to ensure
safety there.

`BitIdx` is transformed to `BitPos` through the `BitOrder` trait, which has an
associated verification function to prove that implementations are correct.
`BitPos` is the only type that can describe memory operations, and is used to
create selection masks of `BitSel` and `BitMask`.
!*/

use crate::{
	mem::BitMemory,
	order::BitOrder,
	store::BitStore,
};

use core::{
	any::type_name,
	fmt::{
		self,
		Binary,
		Debug,
		Display,
		Formatter,
	},
	iter::{
		FusedIterator,
		Sum,
	},
	marker::PhantomData,
	ops::{
		BitAnd,
		BitOr,
		Not,
	},
};

use radium::marker::BitOps;

#[cfg(feature = "serde")]
use core::convert::TryFrom;

macro_rules! make {
	(idx $e:expr) => {
		BitIdx {
			idx: $e,
			_ty: PhantomData,
			}
	};
	(tail $e:expr) => {
		BitTail {
			end: $e,
			_ty: PhantomData,
			}
	};
	(pos $e:expr) => {
		BitPos {
			pos: $e,
			_ty: PhantomData,
			}
	};
	(sel $e:expr) => {
		BitSel { sel: $e }
	};
	(mask $e:expr) => {
		BitMask { mask: $e }
	};
}

/// Marks that an integer can be used in a processor register.
pub trait BitRegister: BitMemory + BitOps + BitStore {}

macro_rules! register {
	($($t:ty),+ $(,)?) => { $(
		impl BitRegister for $t {
		}
	)* };
}

register!(u8, u16, u32);

/** `u64` can only be used as a register on processors whose word size is at
least 64 bits.

This implementation is not present on targets with 32-bit processor words.
**/
#[cfg(target_pointer_width = "64")]
impl BitRegister for u64 {
}

register!(usize);

/** A semantic index of a single bit within a register `R`.

This type is a counter in the range `0 .. R::BITS`, and marks the semantic
position of a bit according to some [`BitOrder`] implementation. As an abstract
counter, it can be used in arithmetic without having to go through `BitOrder`
translation to an electrical position.

# Type Parameters

- `R`: The register type that these values govern.

# Validity

Values of this type are required to be in the range `0 .. R::BITS`. Any value
outside this range will cause the program state to become invalid, and the
library’s behavior is unspecified. The library will never produce such an
invalid value.

# Construction

This type cannot be constructed outside the `bitvec` crate. `bitvec` will
construct safe values of this type, and allows users to view them and use them
to construct other index types from them. All values of this type constructed by
`bitvec` are known to be correct based on user input to the crate.
**/
// #[rustc_layout_scalar_valid_range_end(R::BITS)]
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitIdx<R>
where R: BitRegister
{
	/// Semantic index counter within a register, constrained to `0 .. R::BITS`.
	idx: u8,
	/// Marker for the indexed type.
	_ty: PhantomData<R>,
}

impl<R> BitIdx<R>
where R: BitRegister
{
	/// The inclusive-maximum index.
	pub(crate) const LAST: Self = make!(idx R::MASK);
	/// The zero index.
	pub(crate) const ZERO: Self = make!(idx 0);

	/// Wraps a counter value as a known-good index into an `R` register.
	///
	/// # Parameters
	///
	/// - `idx`: A semantic index of a bit within an `R` register.
	///
	/// # Returns
	///
	/// If `idx` is outside the valid range `0 .. R::BITS`, this returns `None`;
	/// otherwise, it returns a `BitIdx` wrapping the `idx` value.
	#[inline]
	#[doc(hidden)]
	pub(crate) fn new(idx: u8) -> Option<Self> {
		if idx >= R::BITS {
			return None;
		}
		Some(make!(idx idx))
	}

	/// Wraps a counter value as an assumed-good index into an `R` register.
	///
	/// # Parameters
	///
	/// - `idx`: A semantic index of a bit within an `R` register.
	///
	/// # Returns
	///
	/// `idx` wrapped in a `BitIdx`.
	///
	/// # Safety
	///
	/// `idx` **must** be within the valid range `0 .. R::BITS`. In debug
	/// builds, invalid `idx` values cause a panic; release builds do not check
	/// the input.
	#[inline]
	#[doc(hidden)]
	pub unsafe fn new_unchecked(idx: u8) -> Self {
		debug_assert!(
			idx < R::BITS,
			"Bit index {} cannot exceed type width {}",
			idx,
			R::BITS
		);
		make!(idx idx)
	}

	/// Increments an index counter, wrapping at the back edge of the register.
	///
	/// # Parameters
	///
	/// - `self`: The index to increment.
	///
	/// # Returns
	///
	/// - `.0`: The next index after `self`.
	/// - `.1`: Indicates that the new index is in the next register.
	#[inline]
	pub(crate) fn incr(self) -> (Self, bool) {
		let next = self.idx + 1;
		(make!(idx next & R::MASK), next == R::BITS)
	}

	/// Decrements an index counter, wrapping at the front edge of the register.
	///
	/// # Parameters
	///
	/// - `self`: The inedx to decrement.
	///
	/// # Returns
	///
	/// - `.0`: The previous index before `self`.
	/// - `.1`: Indicates that the new index is in the previous register.
	#[inline]
	pub(crate) fn decr(self) -> (Self, bool) {
		let next = self.idx.wrapping_sub(1);
		(make!(idx next & R::MASK), self.idx == 0)
	}

	/// Computes the bit position corresponding to `self` under some ordering.
	///
	/// This forwards to `O::at::<R>`, and is the only public, safe, constructor
	/// for a position counter.
	#[inline(always)]
	pub fn position<O>(self) -> BitPos<R>
	where O: BitOrder {
		O::at::<R>(self)
	}

	/// Computes the bit selector corresponding to `self` under an ordering.
	///
	/// This forwards to `O::select::<R>`, and is the only public, safe,
	/// constructor for a bit selector.
	#[inline(always)]
	pub fn select<O>(self) -> BitSel<R>
	where O: BitOrder {
		O::select::<R>(self)
	}

	/// Computes the bit selector for `self` as an accessor mask.
	///
	/// This is a type-cast over `Self::select`. It is one of the few public,
	/// safe, constructors of a multi-bit mask.
	#[inline]
	pub fn mask<O>(self) -> BitMask<R>
	where O: BitOrder {
		self.select::<O>().mask()
	}

	/// Views the internal index value.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn value(self) -> u8 {
		self.idx
	}

	/// Ranges over all possible index values.
	#[inline]
	pub(crate) fn range_all() -> impl Iterator<Item = Self>
	+ DoubleEndedIterator
	+ ExactSizeIterator
	+ FusedIterator {
		(Self::ZERO.idx ..= Self::LAST.idx).map(|val| make!(idx val))
	}

	/// Constructs a range over all indices between a start and end point.
	///
	/// Because implementation details of the `RangeOps` family are not yet
	/// stable, and heterogenous ranges are not supported, this must be an
	/// opaque iterator rather than a direct `Range<BitIdx<R>>`.
	///
	/// # Parameters
	///
	/// - `from`: The inclusive low bound of the range. This will be the first
	///   index produced by the iterator.
	/// - `upto`: The exclusive high bound of the range. The iterator will halt
	///   before yielding an index of this value.
	///
	/// # Returns
	///
	/// An opaque iterator that is equivalent to the range `from .. upto`.
	///
	/// # Requirements
	///
	/// `from` must be no greater than `upto`.
	#[inline]
	pub fn range(
		from: Self,
		upto: BitTail<R>,
	) -> impl Iterator<Item = Self>
	+ DoubleEndedIterator
	+ ExactSizeIterator
	+ FusedIterator
	{
		debug_assert!(
			from.value() <= upto.value(),
			"Ranges must run from low to high"
		);
		(from.value() .. upto.value()).map(|val| make!(idx val))
	}

	/// Computes the the jump distance for a number of bits away from a start.
	///
	/// This produces the number of elements to move from the starting point,
	/// and then the bit index of the destination bit in the destination
	/// element.
	///
	/// # Parameters
	///
	/// - `self`: A bit index in some memory element, used as the starting
	///   position for the offset calculation.
	/// - `by`: The number of bits by which to move. Negative values go towards
	///   the zero bit index and element address; positive values go towards the
	///   maximal bit index and element address.
	///
	/// # Returns
	///
	/// - `.0`: The number of elements by which to offset the caller’s element
	///   address. This value can be passed directly into [`ptr::offset`].
	/// - `.1`: The bit index of the destination bit in the element selected by
	///   applying the `.0` pointer offset.
	///
	/// [`ptr::offset`]: https://doc.rust-lang.org/std/primitive.pointer.html#method.offset
	#[inline]
	pub(crate) fn offset(self, by: isize) -> (isize, Self) {
		let val = self.value();

		/* Signed-add `*self` and the jump distance. Overflowing is the unlikely
		branch. The result is a bit index, and an overflow marker. `far` is
		permitted to be negative; this means that it is lower in memory than the
		origin bit. The number line has its origin at the front edge of the
		origin element, so `-1` is the *last* bit of the prior memory element.
		*/
		let (far, ovf) = by.overflowing_add(val as isize);
		//  If the `isize` addition does not overflow, then the sum can be used
		//  directly.
		if !ovf {
			//  If `far` is in the origin element, then the jump moves zero
			//  elements and produces `far` as an absolute index directly.
			if (0 .. R::BITS as isize).contains(&far) {
				(0, make!(idx far as u8))
			}
			/* Otherwise, downshift the bit distance to compute the number of
			elements moved in either direction, and mask to compute the absolute
			bit index in the destination element.
			*/
			else {
				(far >> R::INDX, make!(idx far as u8 & R::MASK))
			}
		}
		else {
			/* Overflowing `isize` addition happens to produce ordinary `usize`
			addition. In point of fact, `isize` addition and `usize` addition
			are the same machine instruction to perform the sum; it is merely
			the signed interpretation of the sum that differs. The sum can be
			recast back to `usize` without issue.
			*/
			let far = far as usize;
			//  This is really only needed in order to prevent sign-extension of
			//  the downshift; once shifted, the value can be safely re-signed.
			((far >> R::INDX) as isize, make!(idx far as u8 & R::MASK))
		}
	}

	/// Computes the span information for a region beginning at `self` for `len`
	/// bits.
	///
	/// The span information is the number of elements in the region that hold
	/// live bits, and the position of the tail marker after the live bits.
	///
	/// This forwards to [`BitTail::span`], as the computation is identical for
	/// the two types. Beginning a span at any `Idx` is equivalent to beginning
	/// it at the tail of a previous span.
	///
	/// # Parameters
	///
	/// - `self`: The start bit of the span.
	/// - `len`: The number of bits in the span.
	///
	/// # Returns
	///
	/// - `.0`: The number of elements, starting in the element that contains
	///   `self`, that contain live bits of the span.
	/// - `.1`: The tail counter of the span’s end point.
	#[inline]
	pub(crate) fn span(self, len: usize) -> (usize, BitTail<R>) {
		make!(tail self.value()).span(len)
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Binary for BitIdx<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "{:0>1$b}", self.idx, R::INDX as usize)
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitIdx<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitIdx<{}>(", type_name::<R>())?;
		Display::fmt(&self.idx, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Display for BitIdx<R>
where R: BitRegister
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Display::fmt(&self.idx, fmt)
	}
}

/// Represents an error encountered in `TryFrom<u8> for BitIdx<R>`.
#[repr(transparent)]
#[cfg(feature = "serde")]
pub struct BitIdxErr<R>
where R: BitRegister
{
	/// The value that cannot be wrapped into a `BitIdx`.
	err: u8,
	/// The register type marker.
	_ty: PhantomData<R>,
}

#[cfg(feature = "serde")]
impl<R> TryFrom<u8> for BitIdx<R>
where R: BitRegister
{
	type Error = BitIdxErr<R>;

	#[inline]
	fn try_from(idx: u8) -> Result<Self, Self::Error> {
		Self::new(idx).ok_or(BitIdxErr {
			err: idx,
			_ty: PhantomData,
		})
	}
}

#[cfg(feature = "serde")]
#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitIdxErr<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitIdxErr<{}>(", type_name::<R>())?;
		Display::fmt(&self.err, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(feature = "serde")]
#[cfg(not(tarpaulin_include))]
impl<R> Display for BitIdxErr<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(
			fmt,
			"The value {} is too large to index into {}",
			self.err,
			core::any::type_name::<R>()
		)
	}
}

#[cfg(all(feature = "serde", feature = "std"))]
#[cfg(not(tarpaulin_include))]
impl<R> std::error::Error for BitIdxErr<R> where R: BitRegister
{
}

/** Semantic index of a dead bit *after* a live region.

Like `BitIdx<R>`, this type indicates a semantic counter within a register `R`.
However, it marks the position of a *dead* bit *after* a live range. This means
that it is permitted to have the value of `R::BITS`, to indicate that a live
region touches the semantic back edge of the register `R`.

Instances of this type will only contain the value `0` when the span that
created them is empty. Otherwise, they will have the range `1 ..= R::BITS`.

This type cannot be used for indexing into a register `R`, and does not
translate to a `BitPos<R>`. It has no behavior other than viewing its internal
counter for region arithmetic.

# Type Parameters

- `R`: The register type that these values govern.

# Validity

Values of this type are required to be in the range `0 ..= R::BITS`. Any value
outside this range will cause the program state to become invalid, and the
library’s behavior is unspecified. The library will never produce such an
invalid value.

# Construction

This type cannot be directly constructed outside the `bitvec` crate. `bitvec`
will construct safe values of this type, and allows users to view them and use
them for region computation. All values of this type constructed by `bitvec` are
known to be correct based on user input to the crate.
**/
// #[rustc_layout_scalar_valid_range_end(R::BITS + 1)]
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitTail<R>
where R: BitRegister
{
	/// Semantic tail counter of a register, constrained to `0 ..= R::BITS`.
	end: u8,
	/// Marker for the tailed type.
	_ty: PhantomData<R>,
}

impl<R> BitTail<R>
where R: BitRegister
{
	/// The inclusive-maximum tail counter.
	pub(crate) const END: Self = make!(tail R::BITS);
	/// The zero tail.
	pub(crate) const ZERO: Self = make!(tail 0);

	/// Wraps a counter value as an assumed-good tail of an `R` register.
	///
	/// # Parameters
	///
	/// - `end`: A semantic index of a dead bit in or after an `R` register.
	///
	/// # Returns
	///
	/// `end` wrapped in a `BitTail`.
	///
	/// # Safety
	///
	/// `end` **must** be within the valid range `0 ..= R::BITS`. In debug
	/// builds, invalid `end` values cause a panic; release builds do not check
	/// the input.
	#[inline]
	pub(crate) unsafe fn new_unchecked(end: u8) -> Self {
		debug_assert!(
			end <= R::BITS,
			"Bit tail {} cannot exceed type width {}",
			end,
			R::BITS
		);
		make!(tail end)
	}

	/// Views the internal tail value.
	#[inline]
	#[cfg(not(tarpaulin_include))]
	pub fn value(self) -> u8 {
		self.end
	}

	/// Ranges over all valid tails for a starting index.
	#[inline]
	#[cfg(test)]
	pub(crate) fn range_from(
		start: BitIdx<R>,
	) -> impl Iterator<Item = Self>
	+ DoubleEndedIterator
	+ ExactSizeIterator
	+ FusedIterator {
		(start.idx ..= Self::END.end).map(|val| make!(tail val))
	}

	/// Computes span information for a region beginning immediately after a
	/// preceding region.
	///
	/// The computed region of `len` bits has its start at the *live* bit that
	/// corresponds to the `self` dead tail. The return value is the number of
	/// memory elements containing live bits of the computed span and its tail
	/// marker.
	///
	/// # Parameters
	///
	/// - `self`
	/// - `len`: The number of live bits in the span starting after `self`.
	///
	/// # Returns
	///
	/// - `.0`: The number of elements `R` that contain live bits in the
	///   computed region.
	/// - `.1`: The tail counter of the first dead bit after the new span.
	///
	/// # Behavior
	///
	/// If `len` is `0`, this returns `(0, self)`, as the span has no live bits.
	/// If `self` is `BitTail::END`, then the new region starts at
	/// `BitIdx::ZERO` in the next element.
	pub(crate) fn span(self, len: usize) -> (usize, Self) {
		if len == 0 {
			return (0, self);
		}

		let val = self.end;

		let head = val & R::MASK;
		let bits_in_head = (R::BITS - head) as usize;

		if len <= bits_in_head {
			return (1, make!(tail head + len as u8));
		}

		let bits_after_head = len - bits_in_head;
		let elts = bits_after_head >> R::INDX;
		let tail = bits_after_head as u8 & R::MASK;

		let is_zero = (tail == 0) as u8;
		let edges = 2 - is_zero as usize;
		(elts + edges, make!(tail(is_zero << R::INDX) | tail))
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitTail<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitTail<{}>(", type_name::<R>())?;
		Display::fmt(&self.end, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Display for BitTail<R>
where R: BitRegister
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Display::fmt(&self.end, fmt)
	}
}

/** An electrical position of a single bit within a register `R`.

This type is used as the shift distance in the expression `1 << shamt`. It is
only produced by the translation of a semantic `BitIdx<R>` according to some
[`BitOrder`] implementation using `BitOrder::at`. It can only be used for the
construction of bit masks used to manipulate a register value during memory
access, and serves no other purpose.

# Type Parameters

- `R`: The register type that these values govern.

# Validity

Values of this type are required to be in the range `0 .. R::BITS`. Any value
outside this range will cause the program state to become invalid, and the
library’s behavior is unspecified. The library will never produce such an
invalid value, and users are required to do the same.

# Construction

This type offers public unsafe constructors. `bitvec` does not offer any public
APIs that take values of this type directly; it always routes through `BitOrder`
implementations. As `BitIdx` will only be constructed from safe, correct,
values, and `BitOrder::at` is the only `BitIdx -> BitPos` transform function,
all constructed `BitPos` values are known to be memory-correct.
**/
// #[rustc_layout_scalar_valid_range_end(R::BITS)]
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitPos<R>
where R: BitRegister
{
	/// Electrical position within a register, constrained to `0 .. R::BITS`.
	pos: u8,
	/// Marker for the positioned type.
	_ty: PhantomData<R>,
}

impl<R> BitPos<R>
where R: BitRegister
{
	/// Wraps a value as a known-good position within an `R` register.
	///
	/// # Parameters
	///
	/// - `pos`: An electrical position of a bit within an `R` register.
	///
	/// # Returns
	///
	/// If `pos` is outside the valid range `0 .. R::BITS`, this returns `None`;
	/// otherwise, it returns a `BitPos` wrapping the `pos` value.
	///
	/// # Safety
	///
	/// This function must only be called within a `BitOrder::at` implementation
	/// which is verified to be correct.
	#[inline]
	pub unsafe fn new(pos: u8) -> Option<Self> {
		//  Reject a position value that is not within the range `0 .. R::BITS`.
		if pos >= R::BITS {
			return None;
		}
		Some(make!(pos pos))
	}

	/// Wraps a value as an assumed-good position within an `R` register.
	///
	/// # Parameters
	///
	/// - `pos`: An electrical position within an `R` register.
	///
	/// # Returns
	///
	/// `pos` wrapped in a `BitPos`.
	///
	/// # Safety
	///
	/// `pos` **must** be within the valid range `0 .. R::BITS`. In debug
	/// builds, invalid `pos` values cause a panic; release builds do not check
	/// the input.
	///
	/// This function must only be called in a correct `BitOrder::at`
	/// implementation.
	#[inline]
	pub unsafe fn new_unchecked(pos: u8) -> Self {
		debug_assert!(
			pos < R::BITS,
			"Bit position {} cannot exceed type width {}",
			pos,
			R::BITS
		);
		make!(pos pos)
	}

	/// Constructs a one-hot selection mask from the position counter.
	///
	/// This is a well-typed `1 << pos`.
	///
	/// # Parameters
	///
	/// - `self`
	///
	/// # Returns
	///
	/// A one-hot mask for `R` selecting the bit specified by `self`.
	#[inline]
	pub fn select(self) -> BitSel<R> {
		make!(sel R::ONE << self.pos)
	}

	/// Constructs an untyped bitmask from the position counter.
	///
	/// This removes the one-hot requirement from the selection mask.
	///
	/// # Parameters
	///
	/// - `self`
	///
	/// # Returns
	///
	/// A mask for `R` selecting only the bit specified by `self`.
	#[inline]
	pub fn mask(self) -> BitMask<R> {
		make!(mask self.select().sel)
	}

	/// Views the internal position value.
	#[inline]
	pub fn value(self) -> u8 {
		self.pos
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitPos<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitPos<{}>(", type_name::<R>())?;
		Display::fmt(&self.pos, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Display for BitPos<R>
where R: BitRegister
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Display::fmt(&self.pos, fmt)
	}
}

/** A one-hot selection mask, to be applied to a register `R`.

This type selects exactly one bit, and is produced by the conversion of a
semantic [`BitIdx`] to a [`BitPos`] through a [`BitOrder`] implementation, and
then applying `1 << pos`. Values of this type are used to select only the bit
specified by a `BitIdx` when performing memory operations.

# Type Parameters

- `R`: The register type that these values govern.

# Validity

Values of this type are required to have exactly one bit set to `1` and all
other bits set to `0`.

# Construction

This type is only constructed from `BitPos` values, which are themselves only
constructed by a chain of known-good `BitIdx` values passed into known-correct
`BitOrder` implementations. As such, `bitvec` can use `BitSel` values with full
confidence that they are correct in the surrounding context.
**/
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitSel<R>
where R: BitRegister
{
	/// The one-hot selector mask.
	sel: R,
}

impl<R> BitSel<R>
where R: BitRegister
{
	/// Wraps a selector value as a known-good selection of an `R` register.
	///
	/// # Parameters
	///
	/// - `sel`: A one-hot selection mask of a bit in an `R` register.
	///
	/// # Returns
	///
	/// If `sel` does not have exactly one bit set, this returns `None`;
	/// otherwise, it returns a `BitSel` wrapping the `sel` value.
	///
	/// # Safety
	///
	/// This function must only be called within a `BitOrder::select`
	/// implementation that is verified to be correct.
	#[inline]
	pub unsafe fn new(sel: R) -> Option<Self> {
		if sel.count_ones() != 1 {
			return None;
		}
		Some(make!(sel sel))
	}

	/// Wraps a selector value as an assumed-good selection of an `R` register.
	///
	/// # Parameters
	///
	/// - `sel`: A one-hot selection mask of a bit in an `R` register.
	///
	/// # Returns
	///
	/// `sel` wrapped in a `BitSel`.
	///
	/// # Safety
	///
	/// `sel` **must** have exactly one bit set high and all others low. In
	/// debug builds, invalid `sel` values cause a panic; release builds do not
	/// check the input.
	///
	/// This function must only be called in a correct `BitOrder::select`
	/// implementation.
	#[inline]
	pub unsafe fn new_unchecked(sel: R) -> Self {
		debug_assert!(
			sel.count_ones() == 1,
			"Selections are required to have exactly one set bit: {:0>1$b}",
			sel,
			R::BITS as usize
		);
		make!(sel sel)
	}

	/// Converts the selector into a bit mask.
	///
	/// This is a type-cast.
	#[inline]
	pub fn mask(self) -> BitMask<R>
	where R: BitRegister {
		make!(mask self.sel)
	}

	/// Views the internal selector value.
	#[inline]
	pub fn value(self) -> R {
		self.sel
	}

	/// Ranges over all possible selector values.
	pub fn range_all() -> impl Iterator<Item = Self>
	+ DoubleEndedIterator
	+ ExactSizeIterator
	+ FusedIterator {
		BitIdx::<R>::range_all().map(|i| make!(pos i.idx).select())
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Binary for BitSel<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "{:0>1$b}", self.sel, R::BITS as usize)
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitSel<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitSel<{}>(", type_name::<R>())?;
		Binary::fmt(&self, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Display for BitSel<R>
where R: BitRegister
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Display::fmt(&self.sel, fmt)
	}
}

/** A multi-bit selection mask.

Unlike [`BitSel`], which enforces a strict one-hot mask encoding, this mask type
permits any number of bits to be set or unset. This is used to accumulate
selections for a batch operation on a register.

# Construction

It is only constructed by accumulating `BitSel` values. The chain of custody for
safe construction in this module and in `order` ensures that all masks that are
applied to register values can be trusted to not cause memory unsafety.
**/
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BitMask<R>
where R: BitRegister
{
	/// A mask of any number of bits to select.
	mask: R,
}

impl<R> BitMask<R>
where R: BitRegister
{
	/// A full mask.
	pub const ALL: Self = make!(mask R::ALL);
	/// An empty mask.
	pub const ZERO: Self = make!(mask R::ZERO);

	/// Wraps any `R` value as a bit-mask.
	///
	/// This constructor is provided to explicitly declare that an operation is
	/// discarding the numeric value of an integer and reading it only as a
	/// bit-mask.
	///
	/// # Parameters
	///
	/// - `mask`: Some integer value
	///
	/// # Returns
	///
	/// `mask` wrapped as a bit-mask, with its numeric context discarded.
	///
	/// # Safety
	///
	/// This function must only be called within a `BitOrder::mask`
	/// implementation which is verified to be correct.
	///
	/// Prefer accumulating `BitSel` values using the `Sum` implementation.
	#[inline]
	pub unsafe fn new(mask: R) -> Self {
		make!(mask mask)
	}

	/// Creates a new mask with a selector bit activated.
	///
	/// # Parameters
	///
	/// - `self`
	/// - `sel`: The selector bit to activate in the new mask.
	///
	/// # Returns
	///
	/// A copy of `self`, with the selector at `sel` activated.
	#[inline]
	pub fn combine(mut self, sel: BitSel<R>) -> Self {
		self.insert(sel);
		self
	}

	/// Inserts a selector into an existing mask.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `sel`: The selector bit to insert into the mask.
	///
	/// # Effects
	///
	/// The selector’s bit in the `self` mask is activated.
	#[inline]
	pub fn insert(&mut self, sel: BitSel<R>) {
		self.mask |= sel.sel;
	}

	/// Tests whether a mask contains a given selector bit.
	///
	/// # Paramters
	///
	/// - `self`
	/// - `sel`: The selector bit to test in the `self` mask.
	///
	/// # Returns
	///
	/// Whether `self` has set the bit at `sel`.
	#[inline]
	pub fn test(self, sel: BitSel<R>) -> bool {
		self.mask & sel.sel != R::ZERO
	}

	/// Views the internal mask value.
	#[inline]
	pub fn value(self) -> R {
		self.mask
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Binary for BitMask<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "{:0>1$b}", self.mask, R::BITS as usize)
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Debug for BitMask<R>
where R: BitRegister
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitMask<{}>(", type_name::<R>())?;
		Binary::fmt(&self, fmt)?;
		fmt.write_str(")")
	}
}

#[cfg(not(tarpaulin_include))]
impl<R> Display for BitMask<R>
where R: BitRegister
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Display::fmt(&self.mask, fmt)
	}
}

impl<R> Sum<BitSel<R>> for BitMask<R>
where R: BitRegister
{
	fn sum<I>(iter: I) -> Self
	where I: Iterator<Item = BitSel<R>> {
		iter.fold(Self::ZERO, Self::combine)
	}
}

impl<R> BitAnd<R> for BitMask<R>
where R: BitRegister
{
	type Output = Self;

	fn bitand(self, rhs: R) -> Self {
		make!(mask self.mask & rhs)
	}
}

impl<R> BitOr<R> for BitMask<R>
where R: BitRegister
{
	type Output = Self;

	fn bitor(self, rhs: R) -> Self {
		make!(mask self.mask | rhs)
	}
}

impl<R> Not for BitMask<R>
where R: BitRegister
{
	type Output = Self;

	fn not(self) -> Self::Output {
		make!(mask !self.mask)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::order::{
		Lsb0,
		Msb0,
	};

	#[test]
	fn index_fns() {
		assert!(BitIdx::<u8>::new(8).is_none());

		for n in 0 .. 8 {
			assert_eq!(
				BitIdx::<u8>::new(n).unwrap().position::<Lsb0>().value(),
				n
			);
		}

		for n in 0 .. 8 {
			assert_eq!(
				BitIdx::<u8>::new(n).unwrap().position::<Msb0>().value(),
				7 - n
			);
		}

		for n in 0 .. 8 {
			assert_eq!(
				BitIdx::<u8>::new(n).unwrap().mask::<Lsb0>().value(),
				1 << n
			);
		}

		for n in 0 .. 8 {
			assert_eq!(
				BitIdx::<u8>::new(n).unwrap().mask::<Msb0>().value(),
				128 >> n
			);
		}

		for n in 0 .. 8 {
			assert_eq!(BitIdx::<u8>::new(n).unwrap().value(), n);
		}
	}

	#[test]
	fn tail_fns() {
		for n in 0 .. 8 {
			let tail: BitTail<u8> = make!(tail n);
			assert_eq!(tail.value(), n);
		}
	}

	#[test]
	fn position_fns() {
		assert!(unsafe { BitPos::<u8>::new(8) }.is_none());

		for n in 0 .. 8 {
			let pos: BitPos<u8> = make!(pos n);
			let mask: BitMask<u8> = make!(mask 1 << n);
			assert_eq!(pos.mask(), mask);
		}
	}

	#[test]
	fn select_fns() {
		assert!(unsafe { BitSel::<u8>::new(1) }.is_some());
		assert!(unsafe { BitSel::<u8>::new(3) }.is_none());

		for (n, sel) in BitSel::<u8>::range_all().enumerate() {
			assert_eq!(sel, make!(sel(1 << n) as u8));
		}
	}

	#[test]
	fn fold_masks() {
		assert_eq!(
			BitSel::<u8>::range_all()
				.map(BitSel::mask)
				.fold(BitMask::<u8>::ZERO, |accum, mask| accum | mask.value()),
			BitMask::<u8>::ALL
		);

		assert_eq!(!BitMask::<u8>::ALL, BitMask::ZERO);
	}

	#[test]
	fn offset() {
		let (elts, idx) =
			BitIdx::<u32>::new(31).unwrap().offset(isize::max_value());
		assert_eq!(elts, (isize::max_value() >> 5) + 1);
		assert_eq!(idx, BitIdx::new(30).unwrap());
	}

	#[test]
	fn span() {
		let start: BitTail<u8> = make!(tail 4);
		assert_eq!(start.span(0), (0, start));

		assert_eq!(start.span(4), (1, make!(tail 8)));
		assert_eq!(start.span(8), (2, start));
	}

	#[test]
	fn walk() {
		let end: BitIdx<u8> = make!(idx 7);
		assert_eq!(end.incr(), (make!(idx 0), true));
		assert_eq!(end.decr(), (make!(idx 6), false));
	}
}
