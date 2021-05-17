/*! Memory modeling.

This module provides a `BitStore` trait, which mediates how handles access
memory and perform analysis on the regions they describe.
!*/

use crate::{
	access::BitAccess,
	index::{
		BitIdx,
		BitMask,
		BitRegister,
	},
	mem,
	order::BitOrder,
};

use core::{
	cell::Cell,
	fmt::Debug,
};

use radium::Radium;

/** Common interface for memory regions.

This trait is implemented on the fundamental integers no wider than the target
processor word size, their `Cell` wrappers, and (if present) their `Atomic`
variants. Users provide this type as a parameter to their data structures in
order to inform the structure of how it may access the memory it describes.

Currently, `bitvec` is only tested on 32- and 64- bit architectures. This means
that `u8`, `u16`, `u32`, and `usize` unconditionally implement `BitStore`, but
`u64` will only do so on 64-bit targets, and will be unavailable on 32-bit
targets. This is a necessary restriction of `bitvec` internals. Please comment
on [Issue #76](https://github.com/myrrlyn/bitvec/issues/76) if this affects you.

Specifically, this has the davantage that a `BitSlice<_, Cell<_>>` knows that it
has a view of memory that will not undergo concurrent modification. As such, it
can forego atomic accesses, and just use ordinary load/store instructions
without fear of causing observable race conditions.

The associated types `Mem` and `Alias` allow implementors to know the register
width of the memory they describe (`Mem`) and to know the aliasing status of the
region.

# Generic Programming

Generic programming with associated types is *hard*, especially when using them,
as in this trait, to implement a closed graph of relationships between types.

For example, this trait is implemented such that for any given type `T`,
`T::Alias::Mem` == `T::Mem` == `T::NoAlias::Mem`, `T::Alias::Alias == T::Alias`,
and `T::NoAlias::NoAlias == T::NoAlias`. Unfortunately, the Rust type system
does not allow these relationships to be described, so generic programming that
performs type transitions will *rapidly* become uncomfortable to use.

Internally, `bitvec` makes use of type-manipulation functions that are known to
be correct with respect to the implementations of `BitStore` in order to ease
implementation of library methods.

You are not expected to do significant programming that is generic over the
`BitStore` memory parameter. When using a concrete type, the compiler will
gladly reduce the abstract type associations into their instantiated selections,
allowing monomorphized code to be *much* more convenient than generic.

If you have a use case that involves generic programming over this trait, and
you are encountering difficulties dealing with the type associations, please
file an issue asking for support in this area.

# Supertraits

This trait has trait requirements that better express its behavior:

- `Sealed` prevents it from being implemented by downstream libraries (`Sealed`
  is a public trait in a private module, that only this crate can name).
- `Sized` instructs the compiler that values of this type can be used as
  immediates.
- `Debug` informs the compiler that other structures using this trait bound can
  correctly derive `Debug`.
  **/
pub trait BitStore: seal::Sealed + Sized + Debug {
	/// The register type that the implementor describes.
	type Mem: BitRegister + Into<Self>;

	/// The modifier type over `Self::Mem` used to perform memory access.
	type Access: BitAccess<Item = Self::Mem>;

	/// A sibling `BitStore` implementor that performs alias-aware memory
	/// access.
	///
	/// While the associated type always has the same `Mem` concrete type as
	/// `Self`, attempting to encode this requirement as `<Mem = Self::Mem>
	/// causes Rust to enter an infinite recursion in the trait solver.
	///
	/// Instead, the two `Radium` bounds inform the compiler that the `Alias` is
	/// irradiant over both the current memory and the destination memory types,
	/// allowing generic type algebra to resolve correctly even though the fact
	/// that `Radium` is only implemented once is not guaranteed.
	type Alias: BitStore + Radium<Item = Self::Mem>;

	/// Marker for the thread safety of the implementor.
	///
	/// This is necessary because `Cell<T: Send>` is `Send`, but `Cell` does not
	/// use synchronization instructions and thus cannot be used for aliased
	/// parallelized memory manipulation.
	#[doc(hidden)]
	type Threadsafe;

	/// Require that all implementors are aligned to their width.
	#[doc(hidden)]
	const __ALIGNED_TO_SIZE: [(); 0];

	/// Require that the `::Alias` associated type has the same width and
	/// alignment as `Self`.
	#[doc(hidden)]
	const __ALIAS_WIDTH: [(); 0];

	/// Copies a memory element into the caller’s local context.
	///
	/// # Parameters
	///
	/// - `&self`
	///
	/// # Returns
	///
	/// A copy of the value at `*self`.
	fn load_value(&self) -> Self::Mem;

	/// Fetches the value of one bit in a memory element.
	///
	/// # Type Parameters
	///
	/// - `O`: A bit ordering.
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `index`: The semantic index of the bit in `*self` to read.
	///
	/// # Returns
	///
	/// The value of the bit in `*self` corresponding to `index`.
	fn get_bit<O>(&self, index: BitIdx<Self::Mem>) -> bool
	where O: BitOrder {
		unsafe { BitMask::new(self.load_value()) }.test(index.select::<O>())
	}

	/// Fetches any number of bits from a memory element.
	///
	/// The mask provided to this method must be constructed from indices that
	/// are valid in the caller’s context. As the mask is already computed by
	/// the caller, this does not take an ordering type parameter.
	///
	/// # Parameters
	///
	/// - `&self`
	/// - `mask`: A mask of any number of bits. This is a selection mask of bits
	///   to read.
	///
	/// # Returns
	///
	/// A copy of the memory element at `*self`, with all bits not selected (set
	/// to `0`) in `mask` erased and all bits selected (set to `1`) in `mask`
	/// preserved.
	#[inline]
	fn get_bits(&self, mask: BitMask<Self::Mem>) -> Self::Mem {
		self.load_value() & mask.value()
	}
}

/// Batch implementation of `BitStore` for appropriate types.
macro_rules! store {
	($($t:ty => $a:ty),+ $(,)?) => { $(
		impl BitStore for $t {
			/// The unsigned integers will only be `BitStore` type parameters
			/// for handles to unaliased memory, following the normal Rust
			/// reference rules.
			type Access = Cell<Self>;

			/// In atomic builds, use `radium`’s best-effort atomic export.
			#[cfg(feature = "atomic")]
			type Alias = $a;

			/// In non-atomic builds, use cell wrappers for aliased access.
			#[cfg(not(feature = "atomic"))]
			type Alias = Cell<Self>;

			type Mem = Self;

			#[doc(hidden)]
			type Threadsafe = Self;

			#[doc(hidden)]
			const __ALIGNED_TO_SIZE: [(); 0] = [(); mem::aligned_to_size::<Self>()];

			#[doc(hidden)]
			const __ALIAS_WIDTH: [(); 0] = [(); mem::cmp_layout::<Self::Mem, Self::Alias>()];

			#[inline(always)]
			fn load_value(&self) -> Self::Mem {
				*self
			}
		}

		#[cfg(feature = "atomic")]
		impl BitStore for $a {
			type Access = Self;

			type Alias = Self;

			type Mem = $t;

			#[doc(hidden)]
			type Threadsafe = Self;

			#[doc(hidden)]
			const __ALIGNED_TO_SIZE: [(); 0] = [(); mem::aligned_to_size::<Self>()];

			#[doc(hidden)]
			const __ALIAS_WIDTH: [(); 0] = [(); mem::cmp_layout::<Self::Mem, Self::Alias>()];

			#[inline(always)]
			fn load_value(&self) -> Self::Mem {
				Self::load(self, core::sync::atomic::Ordering::Relaxed)
			}
		}

		impl seal::Sealed for $t {}

		#[cfg(feature = "atomic")]
		impl seal::Sealed for $a {}
	)+ };
}

store!(
	u8 => radium::types::RadiumU8,
	u16 => radium::types::RadiumU16,
	u32 => radium::types::RadiumU32,
);

#[cfg(target_pointer_width = "64")]
store!(u64 => radium::types::RadiumU64);

store!(usize => radium::types::RadiumUsize);

impl<R> BitStore for Cell<R>
where
	Self: Radium<Item = R>,
	R: BitRegister,
{
	type Access = Self;
	type Alias = Self;
	type Mem = R;
	/// Raw pointers are never threadsafe, so this prevents handles using
	/// `Cell<_>` type parameters from crossing thread boundaries.
	#[doc(hidden)]
	type Threadsafe = *const Self;

	// If these are true for `R: BitRegister`, then they are true for `Cell<R>`.

	#[doc(hidden)]
	const __ALIAS_WIDTH: [(); 0] = [];
	#[doc(hidden)]
	const __ALIGNED_TO_SIZE: [(); 0] = [];

	#[inline(always)]
	fn load_value(&self) -> Self::Mem {
		self.get()
	}
}

impl<R> seal::Sealed for Cell<R> where R: BitRegister
{
}

#[cfg(not(any(target_pointer_width = "32", target_pointer_width = "64")))]
compile_fail!(concat!(
	"This architecture is currently not supported. File an issue at ",
	env!("CARGO_PKG_REPOSITORY")
));

/// Enclose the `Sealed` trait against client use.
mod seal {
	/// Marker trait to seal `BitStore` against downstream implementation.
	///
	/// This trait is public in the module, so that other modules in the crate
	/// can use it, but so long as it is not exported by the crate root and this
	/// module is private, this trait effectively forbids downstream
	/// implementation of the `BitStore` trait.
	#[doc(hidden)]
	pub trait Sealed {}
}

#[cfg(test)]
#[cfg(not(tarpaulin_include))]
mod tests {
	use crate::prelude::*;
	use core::cell::Cell;
	use static_assertions::*;

	#[test]
	fn traits() {
		//  The integers are threadsafe, as they are known to be unaliased.
		assert_impl_all!(BitSlice<LocalBits, u8>: Send, Sync);
		assert_impl_all!(BitSlice<LocalBits, u16>: Send, Sync);
		assert_impl_all!(BitSlice<LocalBits, u32>: Send, Sync);
		assert_impl_all!(BitSlice<LocalBits, usize>: Send, Sync);

		#[cfg(target_pointer_width = "64")]
		assert_impl_all!(BitSlice<LocalBits, u64>: Send, Sync);

		//  The integer alias is threadsafe when atomics are enabled.
		#[cfg(feature = "atomic")]
		{
			assert_impl_all!(BitSlice<LocalBits, <u8 as BitStore>::Alias>: Send, Sync);
			assert_impl_all!(BitSlice<LocalBits, <u16 as BitStore>::Alias>: Send, Sync);
			assert_impl_all!(BitSlice<LocalBits, <u32 as BitStore>::Alias>: Send, Sync);
			assert_impl_all!(BitSlice<LocalBits, <usize as BitStore>::Alias>: Send, Sync);

			#[cfg(target_pointer_width = "64")]
			assert_impl_all!(BitSlice<LocalBits, <u64 as BitStore>::Alias>: Send, Sync);
		}

		//  The integer alias is thread unsafe when atomics are disabled.
		#[cfg(not(feature = "atomic"))]
		{
			assert_not_impl_any!(BitSlice<LocalBits, <u8 as BitStore>::Alias>: Send, Sync);
			assert_not_impl_any!(BitSlice<LocalBits, <u16 as BitStore>::Alias>: Send, Sync);
			assert_not_impl_any!(BitSlice<LocalBits, <u32 as BitStore>::Alias>: Send, Sync);
			assert_not_impl_any!(BitSlice<LocalBits, <usize as BitStore>::Alias>: Send, Sync);

			#[cfg(target_pointer_width = "64")]
			assert_not_impl_any!(BitSlice<LocalBits, <u64 as BitStore>::Alias>: Send, Sync);
		}

		//  `Cell`s are never threadsafe.
		assert_not_impl_any!(BitSlice<LocalBits, Cell<u8>>: Send, Sync);
		assert_not_impl_any!(BitSlice<LocalBits, Cell<u16>>: Send, Sync);
		assert_not_impl_any!(BitSlice<LocalBits, Cell<u32>>: Send, Sync);
		assert_not_impl_any!(BitSlice<LocalBits, Cell<usize>>: Send, Sync);

		#[cfg(target_pointer_width = "64")]
		assert_not_impl_any!(BitSlice<LocalBits, Cell<u64>>: Send, Sync);
	}
}
