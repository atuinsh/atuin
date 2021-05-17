/*! A fixed-size region viewed as individual bits, corresponding to `[bool]`.

You can read the language’s [array fundamental documentation][std] here.

This module defines the [`BitArray`] immediate type, and its associated support
code.

`BitArray` has little behavior or properties in its own right. It serves solely
as a type capable of being used in immediate value position, and delegates to
`BitSlice` for all actual work.

[`BitArray`]: struct.BitArray.html
[std]: https://doc.rust-lang.org/std/primitive.array.html
!*/

use crate::{
	order::{
		BitOrder,
		Lsb0,
	},
	slice::BitSlice,
	view::BitView,
};

use core::{
	marker::PhantomData,
	mem::MaybeUninit,
	slice,
};

/* Note on C++ `std::bitset<N>` compatibility:

The ideal API for `BitArray` is as follows:

```rust
struct BitArray<O, T, const N: usize>
where
  O: BitOrder,
  T: BitStore,
  N < T::MAX_BITS,
{
  _ord: PhantomData<O>,
  data: [T; crate::mem::elts::<T>(N)],
}

impl<O, T, const N: usize> BitArray<O, T, N>
where
  O: BitOrder,
  T: BitStore,
{
  pub fn len(&self) -> usize { N }
}
```

This allows the structure to be parametric over the number of bits, rather than
a scalar or array type that satisfies the number of bits. Unfortunately, it is
inexpressible until the Rust compiler’s const-evaluation engine permits using
numeric type parameters in type-level expressions.
*/

/** An array of individual bits, able to be held by value on the stack.

This type is generic over all `Sized` implementors of the `BitView` trait. Due
to limitations in the Rust language’s const-generics implementation (it is both
unstable and incomplete), this must take an array type parameter, rather than a
bit-count integer parameter, making it inconvenient to use. The [`bitarr!`]
macro is capable of constructing both values and specific types of `BitArray`,
and this macro should be preferred for most use.

The advantage of using this wrapper is that it implements `Deref`/`Mut` to
`BitSlice`, as well as implementing all of `BitSlice`’s traits by forwarding to
the bit-slice view of its contained data. This allows it to have `BitSlice`
behavior by itself, without requiring explicit `.as_bitslice()` calls in user
code.

> Note: Not all traits may be implemented for forwarding, as a matter of effort
> and perceived need. Please file an issue for any additional traits that you
> need to be forwarded.

# Limitations

This always produces a bit-slice that fully spans its data; you cannot produce,
for example, an array of twelve bits.

# Type Parameters

- `O`: The ordering of bits within memory elements.
- `V`: Some amount of memory which can be used as the basis for a `BitSlice`
  view. This will usually be an array `[T: BitStore; N]`.

# Examples

This type is useful for marking that some value is always to be used as a
bit-slice.

**/
/// ```rust
/// use bitvec::prelude::*;
///
/// struct HasBitfields {
///   header: u32,
///   //  creates a type declaration
///   fields: bitarr!(for 20, in Msb0, u8),
/// }
///
/// impl HasBitfields {
///   pub fn new() -> Self {
///     Self {
///       header: 0,
///       //  creates a value object. the type paramaters must be repeated.
///       fields: bitarr![Msb0, u8; 0; 20],
///     }
///   }
///
///   /// Access a bit region directly
///   pub fn get_subfield(&self) -> &BitSlice<Msb0, u8> {
///     &self.fields[.. 4]
///   }
///
///   /// Read a 12-bit value out of a region
///   pub fn read_value(&self) -> u16 {
///     self.fields[4 .. 16].load()
///   }
///
///   /// Write a 12-bit value into a region
///   pub fn set_value(&mut self, value: u16) {
///     self.fields[4 .. 16].store(value);
///   }
/// }
/// ```
/**
# Eventual Obsolescence

When const-generics stabilize, this will be modified to have a signature more
like `BitArray<O, T: BitStore, const N: usize>([T; elts::<T>(N)]);`, to mirror
the behavior of ordinary arrays `[T; N]` as they stand today.

[`bitarr!`]: ../../macro.bitarr.html
**/
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct BitArray<O = Lsb0, V = usize>
where
	O: BitOrder,
	V: BitView + Sized,
{
	/// Bit ordering when viewed as a bitslice.
	_ord: PhantomData<O>,
	/// The wrapped data store.
	data: V,
}

impl<O, V> BitArray<O, V>
where
	O: BitOrder,
	V: BitView + Sized,
{
	/// Constructs a new `BitArray` with zeroed memory.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn zeroed() -> Self {
		Self {
			_ord: PhantomData,
			data: unsafe { MaybeUninit::zeroed().assume_init() },
		}
	}

	/// Constructs a new `BitArray` from a data store.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	/// ```
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn new(data: V) -> Self {
		Self {
			_ord: PhantomData,
			data,
		}
	}

	/// Removes the bit-array wrapper, returning the contained data.
	///
	/// # Examples
	///
	/// ```rust
	/// use bitvec::prelude::*;
	///
	/// let bitarr: BitArray<LocalBits, [usize; 1]> = bitarr![0; 30];
	/// let native: [usize; 1] = bitarr.unwrap();
	/// ```
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn unwrap(self) -> V {
		self.data
	}

	/// Views the array as a bit-slice.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_bitslice(&self) -> &BitSlice<O, V::Store> {
		self.data.view_bits::<O>()
	}

	/// Views the array as a mutable bit-slice.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_bitslice(&mut self) -> &mut BitSlice<O, V::Store> {
		self.data.view_bits_mut::<O>()
	}

	/// Views the array as a slice of its underlying elements.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_slice(&self) -> &[V::Store] {
		unsafe {
			slice::from_raw_parts(
				&self.data as *const V as *const V::Store,
				V::const_elts(),
			)
		}
	}

	/// Views the array as a mutable slice of its underlying elements.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_mut_slice(&mut self) -> &mut [V::Store] {
		unsafe {
			slice::from_raw_parts_mut(
				&mut self.data as *mut V as *mut V::Store,
				V::const_elts(),
			)
		}
	}

	/// Views the array as a slice of its raw underlying memory type.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_raw_slice(&self) -> &[V::Mem] {
		unsafe {
			slice::from_raw_parts(
				&self.data as *const V as *const V::Mem,
				V::const_elts(),
			)
		}
	}

	/// Views the array as a mutable slice of its raw underlying memory type.
	#[inline(always)]
	#[cfg(not(tarpaulin_include))]
	pub fn as_raw_mut_slice(&mut self) -> &mut [V::Mem] {
		unsafe {
			slice::from_raw_parts_mut(
				&mut self.data as *mut V as *mut V::Mem,
				V::const_elts(),
			)
		}
	}
}

mod ops;
mod traits;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::prelude::*;

	#[test]
	fn create_arrays() {
		macro_rules! make {
			($($elts:literal),+ $(,)?) => { $(
				let _ = BitArray::<LocalBits, [u8; $elts]>::zeroed();
				let _ = BitArray::<LocalBits, [u16; $elts]>::zeroed();
				let _ = BitArray::<LocalBits, [u32; $elts]>::zeroed();
				let _ = BitArray::<LocalBits, [usize; $elts]>::zeroed();

				#[cfg(target_pointer_width = "64")] {
				let _ = BitArray::<LocalBits, [u64; $elts]>::zeroed();
				}
			)+ };
		}

		make!(
			0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
			19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
		);
	}

	#[test]
	fn wrap_unwrap() {
		let data: [u8; 15] = *b"Saluton, mondo!";
		let bits = BitArray::<LocalBits, _>::new(data);
		assert_eq!(bits.unwrap(), data);
	}
}
