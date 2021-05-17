/*! Bitslice pointer encoding

This module defines the in-memory representation of the handle to a `BitSlice`
region. This structure is crate-internal, and defines the methods required to
store a `BitSlice` pointer in memory and retrieve values from it suitable for
work.

Currently, this module is absolutely forbidden for export outside the crate, and
its implementation cannot be relied upon. Future work *may* choose to stabilize
the encoding, and make it available, but this work is not a priority for the
project.
!*/

use crate::{
	access::BitAccess,
	devel as dvl,
	index::{
		BitIdx,
		BitTail,
	},
	mem::BitMemory,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
};

use core::{
	any,
	fmt::{
		self,
		Debug,
		Formatter,
		Pointer,
	},
	marker::PhantomData,
	ptr::{
		self,
		NonNull,
	},
	slice,
};

use wyz::fmt::FmtForward;

/** Pointer to memory with limited typecasting support.

# Type Parameters

- `T`: The referent data type.
**/
#[doc(hidden)]
#[derive(Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Address<T>
where T: BitStore
{
	/// The numeric value of the address.
	addr: usize,
	/// The referent type of data at the address.
	_ty: PhantomData<T>,
}

#[cfg(not(tarpaulin_include))]
impl<T> Address<T>
where T: BitStore
{
	/// Views a numeric address as a typed data address.
	#[inline(always)]
	pub(crate) fn new(addr: usize) -> Self {
		Self {
			addr,
			_ty: PhantomData,
		}
	}

	/// Views the memory address as an access pointer.
	#[inline(always)]
	pub(crate) fn to_access(self) -> *const T::Access {
		self.addr as *const T::Access
	}

	/// Views the memory address as an alias pointer.
	#[inline(always)]
	pub(crate) fn to_alias(self) -> *const T::Alias {
		self.addr as *const T::Alias
	}

	/// Views the memory address as an immutable pointer.
	#[inline(always)]
	pub(crate) fn to_const(self) -> *const T {
		self.addr as *const T
	}

	/// Views the memory address as a mutable pointer.
	#[inline(always)]
	#[allow(clippy::wrong_self_convention)]
	pub(crate) fn to_mut(self) -> *mut T {
		self.addr as *mut T
	}

	/// Gets the numeric value of the address.
	#[inline(always)]
	pub(crate) fn value(self) -> usize {
		self.addr
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Clone for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn clone(&self) -> Self {
		Self { ..*self }
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> From<&T> for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn from(addr: &T) -> Self {
		(addr as *const T).into()
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> From<*const T> for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn from(addr: *const T) -> Self {
		Self::new((addr) as usize)
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> From<&mut T> for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn from(addr: &mut T) -> Self {
		(addr as *mut T).into()
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> From<*mut T> for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn from(addr: *mut T) -> Self {
		Self::new(addr as usize)
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Debug for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		<Self as Pointer>::fmt(&self, fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Pointer for Address<T>
where T: BitStore
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Pointer::fmt(&self.to_const(), fmt)
	}
}

impl<T> Copy for Address<T> where T: BitStore
{
}

/** Bit-precision slice pointer encoding.

Rust slices use a pointer/length encoding to represent regions of memory.
References to slices of data, `&[T]`, have the ABI layout `(*const T, usize)`.

`BitPtr` encodes a base address, a first-bit index, and a length counter, into
the Rust slice reference layout using this structure, permitting `bitvec` to use
an opaque reference type in its implementation of Rust interfaces that require
references, rather than immediate value types.

# Layout

This structure is a more complex version of the `*const T`/`usize` tuple that
Rust uses to represent slices throughout the language. It breaks the pointer and
counter fundamentals into sub-field components. Rust does not have bitfield
syntax, so the below description of the structure layout is in C++.

```cpp
template <typename T>
struct BitPtr {
  uintptr_t ptr_head : __builtin_ctzll(alignof(T));
  uintptr_t ptr_addr : sizeof(uintptr_T) * 8 - __builtin_ctzll(alignof(T));

  size_t len_head : 3;
  size_t len_bits : sizeof(size_t) * 8 - 3;
};
```

This means that the `BitPtr<T>` has three *logical* fields, stored in four
segments across the two *structural* fields of the type. The widths and
placements of each segment are functions of the size of `*const T` and `usize`,
and of the alignment of the `T` referent buffer element type.

# Fields

This section describes the purpose, semantic meaning, and layout of the three
logical fields.

## Base Address

The address of the base element in a memory region is stored in all but the
lowest bits of the `ptr` field. An aligned pointer to `T` will always have its
lowest log<sub>2</sub>(byte width) bits zeroed, so those bits can be used to
store other information, as long as they are erased before dereferencing the
address as a pointer to `T`.

## Head Bit Index

For any referent element type `T`, the selection of a single bit within the
element requires log<sub>2</sub>(byte width) bits to select a byte within the
element `T`, and another three bits to select a bit within the selected byte.

|Type |Alignment|Trailing Zeros|Count Bits|
|:----|--------:|-------------:|---------:|
|`u8` |        1|             0|         3|
|`u16`|        2|             1|         4|
|`u32`|        4|             2|         5|
|`u64`|        8|             3|         6|

The index of the first live bit in the base element is split to have its three
least significant bits stored in the least significant edge of the `len` field,
and its remaining bits stored in the least significant edge of the `ptr` field.

## Length Counter

All but the lowest three bits of the `len` field are used to store a counter of
live bits in the referent region. When this is zero, the region is empty.
Because it is missing three bits, a `BitPtr` has only ⅛ of the index space of
a `usize` value.

# Significant Values

The following values represent significant instances of the `BitPtr` type.

## Null Slice

The fully-zeroed slot is not a valid member of the `BitPtr<T>` type; it is
reserved as the sentinel value for `Option::<BitPtr<T>>::None`.

## Canonical Empty Slice

All pointers with a `bits: 0` logical field are empty. Pointers used to maintain
ownership of heap buffers are not permitted to erase their `addr` field, but
unowning pointers may do so. When an unowning pointer becomes empty, it may
replace its `addr` with the `NonNull::<T>::dangling()` value.

All empty pointers are equivalent to each other.

### Uninhabited Slices

Any empty pointer with a non-`dangling()` base address is considered to be an
uninhabited region.

# Type Parameters

- `T`: The memory type of the referent region. `BitPtr<T>` is a refined `*[T]`
  slice pointer, and operates on memory in terms of the `T` type for access and
  pointer calculation.

# Safety

A `BitPtr` must never be constructed such that the element addressed by
`self.pointer().to_const().offset(self.elements())` causes an addition overflow.
This will be checked in `new()`.

It is difficult to cause an arithmetic overflow with pointer offsets, as most
targets divide the address space such that programs see a highest address of
`0x7FFF…`. This restriction is inherited from restrictions in the distribution
collection libraries, which have studied these problems extensively and are
reasonable sources of trustworthy plagiarism.

# Undefined Behavior

Values of this type are incompatible with slice pointers. Transmutation of these
values into any other type will result in an incorrect program, and permit the
program to begin illegal or undefined behaviors. This type may never be
manipulated in any way by user code outside of the APIs it offers to this crate;
it certainly may not be seen or observed by other crates.
**/
#[repr(C)]
#[derive(Eq, Hash)]
pub struct BitPtr<T>
where T: BitStore
{
	/// Two-element bitfield structure, holding pointer and head information.
	///
	/// This stores a pointer to the zeroth element of the slice, and the high
	/// bits of the head bit cursor. It is typed as a `NonNull<u8>` in order to
	/// provide null-value optimizations to `Option<BitPtr<T>>`, and because the
	/// presence of head-bit cursor information in the lowest bits means the
	/// bit pattern will not uphold alignment properties assumed by
	/// `NonNull<T>`.
	///
	/// This field cannot be treated as an address of the zeroth byte of the
	/// slice domain, because the owning handle’s [`BitOrder`] implementation
	/// governs the bit pattern of the head cursor.
	///
	/// [`BitOrder`]: ../order/trait.BitOrder.html
	ptr: NonNull<u8>,
	/// Two-element bitfield structure, holding bit-count and head-index
	/// information.
	///
	/// This stores the bit count in its highest bits and the low three bits of
	/// the head `BitIdx` in the lowest three bits.
	///
	/// [`BitIdx`]: ../struct.BitIdx.html
	len: usize,
	_ty: PhantomData<*mut T>,
}

impl<T> BitPtr<T>
where T: BitStore
{
	/// The canonical representation of a pointer to an empty region.
	pub(crate) const EMPTY: Self = Self {
		/* Note: this must always construct the `T` dangling pointer, and then
		convert it into a pointer to `u8`. Creating `NonNull::dangling()`
		directly will always instantiate the `NonNull::<u8>::dangling()`
		pointer, which is VERY incorrect for any other `T` typarams.
		*/
		ptr: unsafe {
			NonNull::new_unchecked(NonNull::<T>::dangling().as_ptr() as *mut u8)
		},
		len: 0,
		_ty: PhantomData,
	};
	/// The number of low bits of `self.len` required to hold the low bits of
	/// the head `BitIdx` cursor.
	///
	/// This is always `3`, until Rust tries to target an architecture that does
	/// not have 8-bit bytes.
	pub(crate) const LEN_HEAD_BITS: usize = 3;
	/// Marks the bits of `self.len` that hold part of the `head` logical field.
	pub(crate) const LEN_HEAD_MASK: usize = 0b0111;
	/// Marks the bits of `self.ptr` that hold the `addr` logical field.
	pub(crate) const PTR_ADDR_MASK: usize = !0 << Self::PTR_HEAD_BITS;
	/// The number of low bits of `self.ptr` required to hold the high bits of
	/// the head `BitIdx` cursor.
	pub(crate) const PTR_HEAD_BITS: usize =
		T::Mem::INDX as usize - Self::LEN_HEAD_BITS;
	/// Marks the bits of `self.ptr` that hold part of the `head` logical field.
	pub(crate) const PTR_HEAD_MASK: usize = !Self::PTR_ADDR_MASK;
	/// The inclusive maximum number of bits that a `BitPtr` can cover.
	pub(crate) const REGION_MAX_BITS: usize = !0 >> Self::LEN_HEAD_BITS;
	/// The inclusive maximum number of elements that the region described by a
	/// `BitPtr` can cover in memory.
	///
	/// This is the number of elements required to store `MAX_BITS`, plus one
	/// because a region could start in the middle of its base element and thus
	/// push the final bits into a new element.
	///
	/// Since the region is ⅛th the bit span of a `usize` counter already, this
	/// number is guaranteed to be well below the limits of arithmetic or Rust’s
	/// own constraints on memory region handles.
	pub(crate) const REGION_MAX_ELTS: usize =
		crate::mem::elts::<T::Mem>(Self::REGION_MAX_BITS) + 1;

	/// Constructs an empty `BitPtr` at a bare pointer.
	///
	/// # Parameters
	///
	/// - `addr`: Some allocated address of a `T` element or region.
	///
	/// # Returns
	///
	/// A zero-length `BitPtr` at `addr`.
	///
	/// # Panics
	///
	/// This function panics if `addr` is not well-aligned to `T`. All addresses
	/// received from the Rust allocation system are required to satisfy this
	/// constraint.
	#[cfg(feature = "alloc")]
	pub(crate) fn uninhabited(addr: impl Into<Address<T>>) -> Self {
		let addr = addr.into();
		assert!(
			addr.value().trailing_zeros() as usize >= Self::PTR_HEAD_BITS,
			"Pointer {:p} does not satisfy minimum alignment requirements {}",
			addr.to_const(),
			Self::PTR_HEAD_BITS
		);
		Self {
			ptr: match NonNull::new(addr.to_mut() as *mut u8) {
				Some(nn) => nn,
				None => return Self::EMPTY,
			},
			len: 0,
			_ty: PhantomData,
		}
	}

	/// Constructs a new `BitPtr` from its components.
	///
	/// # Parameters
	///
	/// - `addr`: A well-aligned pointer to a storage element.
	/// - `head`: The bit index of the first live bit in the element under
	///   `*addr`.
	/// - `bits`: The number of live bits in the region the produced `BitPtr<T>`
	///   describes.
	///
	/// # Returns
	///
	/// This returns `None` in the following cases:
	///
	/// - `addr` is the null pointer, or is not adequately aligned for `T`.
	/// - `bits` is greater than `Self::REGION_MAX_BITS`, and cannot be encoded
	///   into a `BitPtr`.
	/// - addr` is so high in the address space that the element slice wraps
	///   around the address space boundary.
	///
	/// # Safety
	///
	/// The caller must provide an `addr` pointer and a `bits` counter which
	/// describe a `[T]` region which is correctly aligned and validly allocated
	/// in the caller’s memory space. The caller is responsible for ensuring
	/// that the slice of memory the produced `BitPtr<T>` describes is all
	/// governable in the caller’s context.
	pub(crate) fn new(
		addr: impl Into<Address<T>>,
		head: BitIdx<T::Mem>,
		bits: usize,
	) -> Option<Self>
	{
		let addr = addr.into();

		if addr.to_const().is_null()
			|| (addr.value().trailing_zeros() as usize) < Self::PTR_HEAD_BITS
			|| bits > Self::REGION_MAX_BITS
		{
			return None;
		}

		let elts = head.span(bits).0;
		let last = addr.to_const().wrapping_add(elts);
		if last < addr.to_const() {
			return None;
		}

		Some(unsafe { Self::new_unchecked(addr, head, bits) })
	}

	/// Creates a new `BitPtr<T>` from its components, without any validity
	/// checks.
	///
	/// # Safety
	///
	/// ***ABSOLUTELY NONE.*** This function *only* packs its arguments into the
	/// bit pattern of the `BitPtr<T>` type. It should only be used in contexts
	/// where a previously extant `BitPtr<T>` was constructed with ancestry
	/// known to have survived [`::new`], and any manipulations of its raw
	/// components are known to be valid for reconstruction.
	///
	/// # Parameters
	///
	/// See [`::new`].
	///
	/// # Returns
	///
	/// See [`::new`].
	///
	/// [`::new`]: #method.new
	#[inline]
	pub(crate) unsafe fn new_unchecked(
		addr: impl Into<Address<T>>,
		head: BitIdx<T::Mem>,
		bits: usize,
	) -> Self
	{
		let (addr, head) = (addr.into(), head.value() as usize);

		let ptr_data = addr.value() & Self::PTR_ADDR_MASK;
		let ptr_head = head >> Self::LEN_HEAD_BITS;

		let len_head = head & Self::LEN_HEAD_MASK;
		let len_bits = bits << Self::LEN_HEAD_BITS;

		let ptr = Address::new(ptr_data | ptr_head);

		Self {
			ptr: NonNull::new_unchecked(ptr.to_mut()),
			len: len_bits | len_head,
			_ty: PhantomData,
		}
	}

	/// Gets the base element address of the referent region.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// The address of the starting element of the memory region. This address
	/// is weakly typed so that it can be cast by call sites to the most useful
	/// access type.
	#[inline]
	pub(crate) fn pointer(&self) -> Address<T> {
		Address::new(self.ptr.as_ptr() as usize & Self::PTR_ADDR_MASK)
	}

	/// Overwrites the data pointer with a new address. This method does not
	/// perform safety checks on the new pointer.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `ptr`: The new address of the `BitPtr<T>`’s domain.
	///
	/// # Safety
	///
	/// None. The invariants of `::new` must be checked at the caller.
	#[inline]
	#[cfg(feature = "alloc")]
	pub(crate) unsafe fn set_pointer(&mut self, addr: impl Into<Address<T>>) {
		let mut addr = addr.into();
		if addr.to_const().is_null() {
			*self = Self::EMPTY;
			return;
		}
		addr.addr &= Self::PTR_ADDR_MASK;
		addr.addr |= self.ptr.as_ptr() as usize & Self::PTR_HEAD_MASK;
		self.ptr = NonNull::new_unchecked(addr.to_mut() as *mut u8);
	}

	/// Gets the starting bit index of the referent region.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A `BitIdx` of the first live bit in the element at the `self.pointer()`
	/// address.
	pub(crate) fn head(&self) -> BitIdx<T::Mem> {
		//  Get the high part of the head counter out of the pointer.
		let ptr = self.ptr.as_ptr() as usize;
		let ptr_head = (ptr & Self::PTR_HEAD_MASK) << Self::LEN_HEAD_BITS;
		//  Get the low part of the head counter out of the length.
		let len_head = self.len & Self::LEN_HEAD_MASK;
		//  Combine and mark as an index.
		unsafe { BitIdx::new_unchecked((ptr_head | len_head) as u8) }
	}

	/// Write a new `head` value into the pointer, with no other effects.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `head`: A new starting index.
	///
	/// # Effects
	///
	/// `head` is written into the `.head` logical field, without affecting
	/// `.addr` or `.bits`.
	#[cfg(feature = "alloc")]
	pub(crate) unsafe fn set_head(&mut self, head: BitIdx<T::Mem>) {
		let head = head.value() as usize;
		let mut ptr = self.ptr.as_ptr() as usize;

		ptr &= Self::PTR_ADDR_MASK;
		ptr |= head >> Self::LEN_HEAD_BITS;
		self.ptr = NonNull::new_unchecked(ptr as *mut u8);

		self.len &= !Self::LEN_HEAD_MASK;
		self.len |= head & Self::LEN_HEAD_MASK;
	}

	/// Gets the number of live bits in the referent region.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A count of how many live bits the region pointer describes.
	#[inline]
	pub(crate) fn len(&self) -> usize {
		self.len >> Self::LEN_HEAD_BITS
	}

	/// Sets the `.bits` logical member to a new value.
	///
	/// # Parameters
	///
	/// - `&mut self`
	/// - `len`: A new bit length. This must not be greater than
	///   `Self::REGION_MAX_BITS`.
	///
	/// # Effects
	///
	/// The `new_len` value is written directly into the `.bits` logical field.
	#[inline]
	pub(crate) unsafe fn set_len(&mut self, new_len: usize) {
		debug_assert!(
			new_len <= Self::REGION_MAX_BITS,
			"Length {} out of range",
			new_len,
		);
		self.len &= Self::LEN_HEAD_MASK;
		self.len |= new_len << Self::LEN_HEAD_BITS;
	}

	/// Gets the three logical components of the pointer.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// - `.0`: The base address of the referent memory region.
	/// - `.1`: The index of the first live bit in the first element of the
	///   region.
	/// - `.2`: The number of live bits in the region.
	#[inline]
	pub(crate) fn raw_parts(&self) -> (Address<T>, BitIdx<T::Mem>, usize) {
		(self.pointer(), self.head(), self.len())
	}

	/// Computes the number of elements, starting at `self.pointer()`, that the
	/// region touches.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// The count of all elements, starting at `self.pointer()`, that contain
	/// live bits included in the referent region.
	pub(crate) fn elements(&self) -> usize {
		//  Find the distance of the last bit from the base address.
		let total = self.len() + self.head().value() as usize;
		//  The element count is always the bit count divided by the bit width,
		let base = total >> T::Mem::INDX;
		//  plus whether any fractional element exists after the division.
		let tail = total as u8 & T::Mem::MASK;
		base + (tail != 0) as usize
	}

	/// Computes the tail index for the first dead bit after the live bits.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A `BitTail` that is the index of the first dead bit after the last live
	/// bit in the last element. This will almost always be in the range `1 ..=
	/// T::Mem::BITS`.
	///
	/// It will be zero only when `self` is empty.
	#[inline]
	pub(crate) fn tail(&self) -> BitTail<T::Mem> {
		let (head, len) = (self.head(), self.len());

		if head.value() == 0 && len == 0 {
			return BitTail::ZERO;
		}

		//  Compute the in-element tail index as the head plus the length,
		//  modulated by the element width.
		let tail = (head.value() as usize + len) & T::Mem::MASK as usize;
		/* If the tail is zero, wrap it to `T::Mem::BITS` as the maximal. This
		upshifts `1` (tail is zero) or `0` (tail is not), then sets the upshift
		on the rest of the tail, producing something in the range
		`1 ..= T::Mem::BITS`.
		*/
		unsafe {
			BitTail::new_unchecked(
				(((tail == 0) as u8) << T::Mem::INDX) | tail as u8,
			)
		}
	}

	/// Increments the `.head` logical field, rolling over into `.addr`.
	///
	/// # Parameters
	///
	/// - `&mut self`
	///
	/// # Effects
	///
	/// Increments `.head` by one. If the increment resulted in a rollover to
	/// `0`, then the `.addr` field is increased to the next `T::Mem` stepping.
	#[inline]
	pub(crate) unsafe fn incr_head(&mut self) {
		//  Increment the cursor, permitting rollover to `T::Mem::BITS`.
		let head = self.head().value() as usize + 1;

		//  Write the low bits into the `.len` field, then discard them.
		self.len &= !Self::LEN_HEAD_MASK;
		self.len |= head & Self::LEN_HEAD_MASK;
		let head = head >> Self::LEN_HEAD_BITS;

		//  Erase the high bits of `.head` from `.ptr`,
		let mut ptr = self.ptr.as_ptr() as usize;
		ptr &= Self::PTR_ADDR_MASK;
		/* Then numerically add the high bits of `.head` into the low bits of
		`.ptr`. If the head increment rolled over into a new element, this will
		have the effect of raising the `.addr` logical field to the next element
		address, in one instruction.
		*/
		ptr += head;
		self.ptr = NonNull::new_unchecked(ptr as *mut u8);
	}

	/// Views the referent memory region as a slice of aliased elements.
	///
	/// This view will cause UB if it is used simultaneously with views of the
	/// referent region that assume full immutability of referent data.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A slice handle over all memory elements this pointer describes.
	///
	/// # Safety
	///
	/// `T` will be marked as `::Alias` where necessary by `BitSlice`, and so
	/// this pointer already contains the aliasing information it needs to be
	/// safe.
	#[inline]
	pub(crate) fn as_aliased_slice<'a>(&self) -> &'a [T::Alias] {
		unsafe {
			slice::from_raw_parts(self.pointer().to_alias(), self.elements())
		}
	}

	/// Reads a bit some distance away from `self`.
	///
	/// # Type Parameters
	///
	/// - `O`: A bit ordering.
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `index`: The bit distance away from `self` at which to read.
	///
	/// # Returns
	///
	/// The value of the bit `index` bits away from `self.head()`, according to
	/// the `O` ordering.
	#[inline]
	pub(crate) unsafe fn read<O>(&self, index: usize) -> bool
	where O: BitOrder {
		let (elt, bit) = self.head().offset(index as isize);
		let base = self.pointer().to_const();
		(&*base.offset(elt)).get_bit::<O>(bit)
	}

	/// Writes a bit some distance away from `self`.
	///
	/// # Type Parameters
	///
	/// - `O`: A bit ordering.
	///
	/// # Parameters
	///
	/// - `&self`: The `self` pointer must be describing a write-capable region.
	/// - `index`: The bit distance away from `self` at which to write,
	///   according to the `O` ordering.
	/// - `value`: The bit value to insert at `index`.
	///
	/// # Effects
	///
	/// `value` is written to the bit specified by `index`, relative to
	/// `self.head()` and `self.pointer()`.
	#[inline]
	pub(crate) unsafe fn write<O>(&self, index: usize, value: bool)
	where O: BitOrder {
		let (elt, bit) = self.head().offset(index as isize);
		let base = self.pointer().to_access();
		(&*base.offset(elt)).write_bit::<O>(bit, value);
	}

	/// Produces the distance, in elements and bits, between two bit-pointers.
	///
	/// # Undefined Behavior
	///
	/// It is undefined to calculate the distance between pointers that are not
	/// part of the same allocation region. This function is defined only when
	/// `self` and `other` are produced from the same region.
	///
	/// # Parameters
	///
	/// - `self`
	/// - `other`: Another `BitPtr<T>`. This function is undefined if it is not
	///   produced from the same region as `self`.
	///
	/// # Returns
	///
	/// - `.0`: The distance in elements between the first element of `self` and
	///   the first element of `other`. Negative if `other` is lower in memory
	///   than `self`; positive if `other` is higher.
	/// - `.1`: The distance in bits between the first bit of `self` and the
	///   first bit of `other`. Negative if `other`’s first bit is lower in its
	///   element than is `self`’s first bit; positive if `other`’s first bit is
	///   higher in its element than is `self`’s first bit.
	///
	/// # Truth Tables
	///
	/// Consider two adjacent bytes in memory. We will define four pairs of
	/// bit-pointers of width `1` at various points in this span in order to
	/// demonstrate the four possible states of difference.
	///
	/// ```text
	///    [ 0 1 2 3 4 5 6 7 ] [ 8 9 a b c d e f ]
	/// 1.       A                       B
	/// 2.             A             B
	/// 3.           B           A
	/// 4.     B                             A
	/// ```
	///
	/// 1. The pointer `A` is in the lower element and `B` is in the higher. The
	///    first bit of `A` is lower in its element than the first bit of `B` is
	///    in its element. `A.ptr_diff(B)` thus produces positive element and
	///    bit distances: `(1, 2)`.
	/// 2. The pointer `A` is in the lower element and `B` is in the higher. The
	///    first bit of `A` is higher in its element than the first bit of `B`
	///    is in its element. `A.ptr_diff(B)` thus produces a positive element
	///    distance and a negative bit distance: `(1, -3)`.
	/// 3. The pointer `A` is in the higher element and `B` is in the lower. The
	///    first bit of `A` is lower in its element than the first bit of `B` is
	///    in its element. `A.ptr_diff(B)` thus produces a negative element
	///    distance and a positive bit distance: `(-1, 4)`.
	/// 4. The pointer `A` is in the higher element and `B` is in the lower. The
	///    first bit of `A` is higher in its element than the first bit of `B`
	///    is in its element. `A.ptr_diff(B)` thus produces negative element and
	///    bit distances: `(-1, -5)`.
	pub(crate) unsafe fn ptr_diff(self, other: Self) -> (isize, i8) {
		let self_ptr = self.pointer();
		let other_ptr = other.pointer();
		//  FIXME(myrrlyn): `core::ptr::offset_from` stabilizes in 1.47.
		//  let elts = other_ptr.to_const().offset_from(self_ptr.to_const());
		let elts = other_ptr
		.value()
		.wrapping_sub(self_ptr.value())
			//  Pointers are byte-addressed, so remember to divide the byte
			//  distance by the element width.
			.wrapping_div(core::mem::size_of::<T>()) as isize;
		let bits = other.head().value() as i8 - self.head().value() as i8;
		(elts, bits)
	}

	/// Typecasts a raw region pointer into a pointer structure.
	#[inline]
	pub(crate) fn from_bitslice_ptr<O>(raw: *const BitSlice<O, T>) -> Self
	where O: BitOrder {
		let slice_nn = match NonNull::new(raw as *const [()] as *mut [()]) {
			Some(r) => r,
			None => return Self::EMPTY,
		};
		let ptr = dvl::nonnull_slice_to_base(slice_nn).cast::<u8>();
		let len = unsafe { slice_nn.as_ref() }.len();
		Self {
			ptr,
			len,
			_ty: PhantomData,
		}
	}

	/// Typecasts a raw region pointer into a pointer structure.
	#[inline(always)]
	#[cfg(feature = "alloc")]
	pub(crate) fn from_bitslice_ptr_mut<O>(raw: *mut BitSlice<O, T>) -> Self
	where O: BitOrder {
		Self::from_bitslice_ptr(raw as *const BitSlice<O, T>)
	}

	/// Type-casts the pointer structure into a raw region pointer.
	#[inline]
	pub(crate) fn to_bitslice_ptr<O>(self) -> *const BitSlice<O, T>
	where O: BitOrder {
		ptr::slice_from_raw_parts(
			self.ptr.as_ptr() as *const u8 as *const (),
			self.len,
		) as *const BitSlice<O, T>
	}

	/// Typecasts the pointer structure into a raw mutable-region pointer.
	#[inline(always)]
	pub(crate) fn to_bitslice_ptr_mut<O>(self) -> *mut BitSlice<O, T>
	where O: BitOrder {
		self.to_bitslice_ptr::<O>() as *mut BitSlice<O, T>
	}

	/// Typecasts the pointer structure into a region reference.
	///
	/// # Safety
	///
	/// This must only be used when the pointer refers to a region that is
	/// correctly initialized in the caller’s context. There must be no `&mut
	/// BitSlice<O, T>` references to the referent region.
	///
	/// # Lifetimes
	///
	/// - `'a`: The minimum lifetime of the referent region, as understood by
	///   the caller.
	#[inline(always)]
	pub(crate) fn to_bitslice_ref<'a, O>(self) -> &'a BitSlice<O, T>
	where O: BitOrder {
		unsafe { &*self.to_bitslice_ptr::<O>() }
	}

	/// Typecasts the pointer structure into a mutable-region reference.
	///
	/// # Safety
	///
	/// This must only be used when the pointer refers to a region that is
	/// correctly initialized *and uniquely mutable* in the caller’s context.
	/// There must be no other references of any kind to the referent region.
	///
	/// # Lifetimes
	///
	/// - `'a`: The minimum lifetime of the referent region, as understood by
	///   the caller.
	#[inline(always)]
	pub(crate) fn to_bitslice_mut<'a, O>(self) -> &'a mut BitSlice<O, T>
	where O: BitOrder {
		unsafe { &mut *self.to_bitslice_ptr_mut::<O>() }
	}

	/// Typecasts the pointer structure into a `NonNull<BitSlice>` pointer.
	///
	/// This function is used by the owning indirect handles, and does not yet
	/// have any purpose in non-`alloc` programs.
	#[inline]
	#[cfg(feature = "alloc")]
	pub(crate) fn to_nonnull<O>(self) -> NonNull<BitSlice<O, T>>
	where
		O: BitOrder,
		T: BitStore,
	{
		unsafe { NonNull::new_unchecked(self.to_bitslice_ptr_mut()) }
	}

	/// Renders the pointer structure into a formatter for use during
	/// higher-level type `Debug` implementations.
	///
	/// # Parameters
	///
	/// - `self`
	/// - `fmt`: The formatter into which the pointer is written.
	/// - `name`: The suffix of the higher-level object rendering its pointer.
	///   The `Bit` prefix is applied to the object type name in this format.
	/// - `ord`: The name of a `BitOrder` type parameter, if any.
	/// - `fields`: Any additional fields in the object’s debuginfo to be
	///   rendered.
	///
	/// # Returns
	///
	/// The result of formatting the pointer into the receiver.
	///
	/// # Behavior
	///
	/// This function writes `Bit{name}<[{ord}, ]T> {{ {fields} }}` into the
	/// `fmt` formatter, where `{fields}` includes the address, head index, and
	/// bit count of the pointer, as well as any additional fields provided by
	/// the caller.
	///
	/// Higher types in the crate should use this function to drive their
	/// `Debug` implementations, and then use `BitSlice`’s list formatters to
	/// display their contents if appropriate.
	#[inline]
	pub(crate) fn render<'a>(
		&'a self,
		fmt: &'a mut Formatter,
		name: &'a str,
		ord: Option<&'a str>,
		fields: impl IntoIterator<Item = &'a (&'a str, &'a dyn Debug)>,
	) -> fmt::Result
	{
		write!(fmt, "Bit{}<", name)?;
		if let Some(ord) = ord {
			write!(fmt, "{}, ", ord)?;
		}
		write!(fmt, "{}>", any::type_name::<T::Mem>())?;
		let mut builder = fmt.debug_struct("");
		builder
			.field("addr", &self.pointer().fmt_pointer())
			.field("head", &self.head().fmt_binary())
			.field("bits", &self.len());
		for (name, value) in fields {
			builder.field(name, value);
		}
		builder.finish()
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Clone for BitPtr<T>
where T: BitStore
{
	fn clone(&self) -> Self {
		Self { ..*self }
	}
}

impl<T, U> PartialEq<BitPtr<U>> for BitPtr<T>
where
	T: BitStore,
	U: BitStore,
{
	fn eq(&self, other: &BitPtr<U>) -> bool {
		let (addr_a, head_a, bits_a) = self.raw_parts();
		let (addr_b, head_b, bits_b) = other.raw_parts();
		//  Since ::BITS is an associated const, the compiler will automatically
		//  replace the entire function with `false` when the types don’t match.
		T::Mem::BITS == U::Mem::BITS
			&& addr_a.value() == addr_b.value()
			&& head_a.value() == head_b.value()
			&& bits_a == bits_b
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Default for BitPtr<T>
where T: BitStore
{
	#[inline(always)]
	fn default() -> Self {
		Self::EMPTY
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Debug for BitPtr<T>
where T: BitStore
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		Pointer::fmt(self, fmt)
	}
}

#[cfg(not(tarpaulin_include))]
impl<T> Pointer for BitPtr<T>
where T: BitStore
{
	#[inline(always)]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		self.render(fmt, "Ptr", None, None)
	}
}

impl<T> Copy for BitPtr<T> where T: BitStore
{
}

#[cfg(test)]
mod tests {
	use crate::{
		bits,
		order::Msb0,
	};

	#[test]
	#[cfg(feature = "alloc")]
	fn render() {
		let bits = bits![Msb0, u8; 0, 1, 0, 0];

		let render = format!("{:?}", bits.bitptr());
		assert!(render.starts_with("BitPtr<u8> { addr: 0x"));
		assert!(render.ends_with(", head: 000, bits: 4 }"));

		let render = format!("{:#?}", bits);
		assert!(render.starts_with("BitSlice<bitvec::order::Msb0, u8> {"));
		assert!(render.ends_with("} [\n    0b0100,\n]"), "{}", render);
	}

	#[test]
	fn ptr_diff() {
		let bits = bits![Msb0, u8; 0; 16];

		let a = bits[2 .. 3].bitptr();
		let b = bits[12 .. 13].bitptr();
		assert_eq!(unsafe { a.ptr_diff(b) }, (1, 2));

		let a = bits[5 .. 6].bitptr();
		let b = bits[10 .. 11].bitptr();
		assert_eq!(unsafe { a.ptr_diff(b) }, (1, -3));

		let a = bits[8 .. 9].bitptr();
		let b = bits[4 .. 5].bitptr();
		assert_eq!(unsafe { a.ptr_diff(b) }, (-1, 4));

		let a = bits[14 .. 15].bitptr();
		let b = bits[1 .. 2].bitptr();
		assert_eq!(unsafe { a.ptr_diff(b) }, (-1, -5));
	}
}
