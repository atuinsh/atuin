/*! Proxy reference for `&mut bool`

`BitSlice` regions can easily produce *read* references to `bool`s that they
contain, by testing the bit and producing the appropriate `&'static bool`, but
Rust’s rules forbid production of a *write* reference to a `bool` stored within
a `BitSlice` region. This is because references `&mut T` must be dereferencable
addresses of type `T`, and `&mut BitSlice` is a non-dereferencable encoded
pointer.

As Rust does not permit defining a method on `&mut _` references that can be
used to store a value into the referenced location, this type must be used
instead. Rust’s strict use of references, rather than arbitrary referential
types, makes this the only major API incompatibility with the rest of the
standard library.
!*/

use crate::{
	access::BitAccess,
	index::BitIdx,
	order::BitOrder,
	slice::BitSlice,
	store::BitStore,
};

use core::{
	fmt::{
		self,
		Debug,
		Formatter,
	},
	marker::PhantomData,
	mem,
	ops::{
		Deref,
		DerefMut,
	},
	ptr::NonNull,
};

use wyz::fmt::FmtForward;

/** Proxy reference type, equivalent to `&mut bool`.

This is a two-word structure capable of correctly referring to a single bit in
a memory element. Because Rust does not permit reference-like objects in the
same manner that C++ does – `&T` and `&mut T` values are required to be
immediately-valid pointers, not objects – `bitvec` cannot manifest encoded
`&mut Bit` values in the same way that it can manifest `&mut BitSlice`.

Instead, this type implements `Deref` and `DerefMut` to an internal `bool` slot,
and in `Drop` commits the value of that `bool` to the proxied bit in the source
`BitSlice` from which the `BitMut` value was created. The combination of Rust’s
own exclusion rules and the aliasing type system in this library ensure that a
`BitMut` value has unique access to the bit it proxies, and the memory element
it uses will not have destructive data races from other views.

# Lifetimes

- `'a`: The lifetime of the source `&'a mut BitSlice` that created the `BitMut`.

# Type Parameters

- `O`: The `BitOrder` type parameter from the source `&mut BitSlice`.
- `T`: The `BitStore` type parameter from the source `&mut BitSlice`.

# Examples

```rust
use bitvec::prelude::*;

let bits = bits![mut 0; 2];

let (left, right) = bits.split_at_mut(1);
let mut first = left.get_mut(0).unwrap();
let second = right.get_mut(0).unwrap();

// Referential behavior
*first = true;
// Direct write
second.set(true);

drop(first); // it’s not a reference!
assert_eq!(bits, bits![1; 2]);
```
**/
pub struct BitMut<'a, O, T>
where
	O: BitOrder,
	T: 'a + BitStore,
{
	/// Accessing pointer to the containing element.
	addr: NonNull<T::Access>,
	/// Index of the proxied bit within the containing element.
	head: BitIdx<T::Mem>,
	/// A local cache for `Deref` usage.
	data: bool,
	/// This type is semantically equivalent to a mutable slice of length 1.
	_ref: PhantomData<&'a mut BitSlice<O, T>>,
}

impl<O, T> BitMut<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	/// Constructs a new proxy from provided element and bit addresses.
	///
	/// # Parameters
	///
	/// - `addr`: The address of a memory element, correctly typed for access.
	/// - `head`: The index of a bit within `*addr`.
	///
	/// # Safety
	///
	/// The caller must produce `addr`’s value from a valid reference, and its
	/// type from the correct access requirements at time of construction.
	#[inline]
	pub(crate) unsafe fn new_unchecked(
		addr: *const T::Access,
		head: BitIdx<T::Mem>,
	) -> Self
	{
		Self {
			_ref: PhantomData,
			addr: NonNull::new_unchecked(addr as *mut T::Access),
			head,
			data: (&*(addr as *const T)).get_bit::<O>(head),
		}
	}

	/// Writes a bit into the proxied location without an intermediate copy.
	///
	/// This function writes `value` directly into the proxied location, and
	/// does not store `value` in the proxy’s internal cache. This should be
	/// equivalent to the behavior seen when using ordinary `DerefMut` proxying,
	/// but the latter depends on compiler optimization.
	///
	/// # Parameters
	///
	/// - `self`: This destroys the proxy, as it becomes invalid when writing
	///   directly to the location without updating the cache.
	/// - `value`: The new bit to write into the proxied slot.
	#[inline]
	pub fn set(mut self, value: bool) {
		self.write(value);
		mem::forget(self);
	}

	/// Commits a bit into memory.
	///
	/// This is the internal function used to drive `.set()` and `.drop()`.
	#[inline]
	fn write(&mut self, value: bool) {
		unsafe { (&*self.addr.as_ptr()).write_bit::<O>(self.head, value) }
	}
}

impl<O, T> Debug for BitMut<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
		write!(fmt, "BitMut<{}>", core::any::type_name::<T::Mem>())?;
		fmt.debug_struct("")
			.field("addr", &self.addr.as_ptr().fmt_pointer())
			.field("head", &self.head.fmt_binary())
			.field("data", &self.data)
			.finish()
	}
}

impl<O, T> Deref for BitMut<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	type Target = bool;

	#[inline]
	fn deref(&self) -> &Self::Target {
		&self.data
	}
}

impl<O, T> DerefMut for BitMut<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline]
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.data
	}
}

impl<O, T> Drop for BitMut<'_, O, T>
where
	O: BitOrder,
	T: BitStore,
{
	#[inline(always)]
	fn drop(&mut self) {
		let value = self.data;
		self.write(value);
	}
}

#[cfg(test)]
mod tests {
	use crate::prelude::*;

	#[test]
	fn proxy_ref() {
		let mut data = 0u32;
		let bits = BitSlice::<Lsb0, _>::from_element_mut(&mut data);
		assert!(!bits[0]);

		let mut proxy = bits.first_mut().unwrap();
		*proxy = true;

		//  We can inspect the cache, but `proxy` locks the entire `bits` for
		//  the duration of its binding, so we cannot observe that the cache is
		//  not written into the main buffer.
		assert!(*proxy);
		drop(proxy);

		//  The proxy commits the cache on drop, releasing its lock on the main
		//  buffer, permitting us to see that the writeback occurred.
		assert!(bits[0]);
		assert_eq!(data, 1);
	}
}
