//! Constructor macros for the crate’s collection types.

#[macro_use]
#[doc(hidden)]
pub mod internal;

/** Constructs a `BitSlice` handle out of a literal array in source code, like
`vec!`.

`bits!` can be invoked in a number of ways. It takes the name of a `BitOrder`
implementation, the name of a `BitStore`-implementing core type (which can be
any of the fundamental integers, their `Cell` wrappers, or their `Atomic`
sibling types), and zero or more expressions which are used to build the bits.
Each value expression corresponds to one bit. If the expression evaluates to
`0`, it is the zero bit; otherwise, it is the `1` bit.

`bits!` can be invoked with no type specifiers, a `BitOrder` specifier only, or
both a `BitOrder` and a `BitStore` specifier. It cannot be invoked with a
`BitStore` but no `BitOrder`, as the macro grammar is incapable of
distinguishing between these two.

In addition, a `mut` marker may be used as the first argument to produce an
`&mut BitSlice` handle instead of a `&BitSlice` handle.

Like `vec!`, `bits!` supports bit lists `[0, 1, …]` and repetition markers
`[1; n]`.

# Examples

```rust
use bitvec::prelude::*;

bits![Msb0, u8; 0, 1];
bits![mut Lsb0, u8; 0, 1,];
bits![Msb0; 0, 1];
bits![mut Lsb0; 0, 1,];
bits![0, 1];
bits![mut 0, 1,];
bits![0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0];
bits![Msb0, u8; 1; 5];
bits![mut Lsb0; 0; 5];
bits![1; 5];
bits![mut LocalBits; 0, 1,];
```
**/
#[macro_export]
macro_rules! bits {
	//  Sequence syntax `[bit (, bit)*]` or `[(bit ,)*]`.

	//  Explicit order and store.

	(mut $order:ident, $store:ident; $($val:expr),* $(,)?) => {{
		&mut $crate::bitarr![$order, $store; $($val),*][.. $crate::__count!($($val),*)]
	}};

	/* These arms differ in `$order:ident` and `$order:path` in order to force
	the matcher to wrap a `:path`, which is `[:tt]`, as a single opaque `:tt`
	for propagation through the macro call. Since the literal `$order` values
	will match as `:ident`, not `:path`, this will only ever enter for orderings
	that the rest of the macros would not be able to inspect and special-case
	*anyway*.
	*/

	(mut $order:path, $store:ident; $($val:expr),* $(,)?) => {{
		&mut $crate::bitarr![$order, $store; $($val),*][.. $crate::__count!($($val),*)]
	}};

	//  Explicit order, default store.

	(mut $order:ident; $($val:expr),* $(,)?) => {
		unsafe { $crate::bits!(mut $order, usize; $($val),*) }
	};

	(mut $order:path; $($val:expr),* $(,)?) => {
		unsafe { $crate::bits!(mut $order, usize; $($val),*) }
	};

	//  Default order and store.

	(mut $($val:expr),* $(,)?) => {
		unsafe { $crate::bits!(mut Lsb0, usize; $($val),*) }
	};

	//  Repetition syntax `[bit ; count]`.
	//  NOTE: `count` must be a `const`, as this is a non-allocating macro.

	//  Explicit order and store.

	(mut $order:ident, $store:ident; $val:expr; $len:expr) => {{
		&mut $crate::bitarr![$order, $store; $val; $len][.. $len]
	}};

	(mut $order:path, $store:ident; $val:expr; $len:expr) => {{
		&mut $crate::bitarr![$order, $store; $val; $len][.. $len]
	}};

	//  Explicit order, default store.

	(mut $order:ident; $val:expr; $len:expr) => {
		$crate::bits!(mut $order, usize; $val; $len)
	};

	(mut $order:path; $val:expr; $len:expr) => {
		$crate::bits!(mut $order, usize; $val; $len)
	};

	//  Default order and store.

	(mut $val:expr; $len:expr) => {
		$crate::bits!(mut Lsb0, usize; $val; $len)
	};

	//  Repeat everything from above, but now immutable.

	($order:ident, $store:ident; $($val:expr),* $(,)?) => {{
		&$crate::bitarr![$order, $store; $($val),*][.. $crate::__count!($($val),*)]
	}};

	($order:path, $store:ident; $($val:expr),* $(,)?) => {{
		&$crate::bitarr![$order, $store; $($val),*][.. $crate::__count!($($val),*)]
	}};

	($order:ident; $($val:expr),* $(,)?) => {
		$crate::bits!($order, usize; $($val),*)
	};

	($order:path; $($val:expr),* $(,)?) => {
		$crate::bits!($order, usize; $($val),*)
	};

	($($val:expr),* $(,)?) => {
		$crate::bits!(Lsb0, usize; $($val),*)
	};

	($order:ident, $store:ident; $val:expr; $len:expr) => {{
		&$crate::bitarr![$order, $store; $val; $len][.. $len]
	}};

	($order:path, $store:ident; $val:expr; $len:expr) => {{
		&$crate::bitarr![$order, $store; $val; $len][.. $len]
	}};

	($order:ident; $val:expr; $len:expr) => {
		$crate::bits!($order, usize; $val; $len)
	};

	($order:path; $val:expr; $len:expr) => {
		$crate::bits!($order, usize; $val; $len)
	};

	($val:expr; $len:expr) => {
		$crate::bits!(Lsb0, usize; $val; $len)
	};
}

/** Constructs a `BitArray` wrapper out of a literal array in source code, like
`bits!`

As with all macro constructors, `bitarr!` can be invoked with either a sequence
of individual bit expressions (`expr, expr`) or a repeated bit (`expr; count`).
Additionally, the bit-ordering and element type can be provided as optional
prefix arguments.

The produced value is of type `BitArray<O, [T; N]>`, and is subject to
[`BitArray`]’s restricitons of element `T` length `N`. For instance, attempting
to produce a bit array that fills more than 32 `T` elements will fail.

In addition, `bitarr!` can be used to produce a type name instead of a value by
using the syntax `bitarr!(for N [, in [O,] T])`. This syntax allows the
production of a monomorphized `BitArray<O, V>` type that is capable of holding
`N` bits. It can be used to type static sites such as `struct` fields and
`const` or `static` declarations, and in these positions must specify both type
arguments as well as the length. It can also be used to type `let`-bindings, but
as type inference is permitted here, it is less useful in this position.

# Examples

```rust
use bitvec::prelude::*;

bitarr![Msb0, u8; 0, 1];
bitarr![Msb0; 0, 1];
bitarr![0, 1];
bitarr![0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0];
bitarr![Msb0, u8; 1; 5];
bitarr![1; 5];
```

This example shows how the `for N, in O, T` syntax can be used to type locations
that cannot use inference:

```rust
use bitvec::prelude::*;

struct ContainsBitfield {
  data: bitarr!(for 10, in Msb0, u8),
}

fn zero() -> ContainsBitfield {
  ContainsBitfield { data: bitarr![Msb0, u8; 0; 10] }
}
```

The order/store type parameters must be repeated in the macros to construct both
the typename and the value. Mismatches will result in a compiler error.
**/
#[macro_export]
macro_rules! bitarr {
	//  Produces a typename instead of a value expression

	(for $len:literal, in $order:path, $store:ident) => {
		$crate::array::BitArray::<
			$order,
			[$store; $crate::mem::elts::<$store>($len)],
		>
	};

	(for $len:literal, in $store:ident) => {
		$crate::bitarr!(for $len, in $crate::order::Lsb0, usize)
	};

	(for $len:literal) => {
		$crate::bitarr!(for $len, in usize)
	};

	//  Produces a value expression

	($order:ident, $store:ident; $($val:expr),* $(,)?) => {
		$crate::array::BitArray::<
			$order,
			[$store; $crate::__count_elts!($store; $($val),*)],
		>::new(
			$crate::__bits_store_array!($order, $store; $($val),*)
		)
	};

	($order:path, $store:ident; $($val:expr),* $(,)?) => {
		$crate::array::BitArray::<
			$order,
			[$store; $crate::__count_elts!($store; $($val),*)],
		>::new(
			$crate::__bits_store_array!($order, $store; $($val),*)
		)
	};

	($order:ident; $($val:expr),* $(,)?) => {
		$crate::bitarr!($order, usize; $($val),*)
	};

	($order:path; $($val:expr),* $(,)?) => {
		$crate::bitarr!($order, usize; $($val),*)
	};

	($($val:expr),* $(,)?) => {
		$crate::bitarr!(Lsb0, usize; $($val),*)
	};

	($order:ident, $store:ident; $val:expr; $len:expr) => {
		$crate::array::BitArray::<
			$order,
			[$store; $crate::mem::elts::<$store>($len)],
		>::new([
			$crate::__extend_bool!($val, $store);
			$crate::mem::elts::<$store>($len)
		])
	};

	($order:path, $store:ident; $val:expr; $len:expr) => {
		$crate::array::BitArray::<
			$order,
			[$store; $crate::mem::elts::<$store>($len)],
		>::new([
			$crate::__extend_bool!($val, $store);
			$crate::mem::elts::<$store>($len)
		])
	};

	($order:ident; $val:expr; $len:expr) => {
		$crate::bitarr!($order, usize; $val; $len)
	};

	($order:path; $val:expr; $len:expr) => {
		$crate::bitarr!($order, usize; $val; $len)
	};

	($val:expr; $len:expr) => {
		$crate::bitarr!(Lsb0, usize; $val; $len)
	};
}

/** Constructs a `BitVec` out of a literal array in source code, like `vec!`.

`bitvec!` can be invoked in a number of ways. It takes the name of a `BitOrder`
implementation, the name of a `BitStore`-implementing fundamental, and zero or
more fundamentals (integer, floating-point, or boolean) which are used to build
the bits. Each fundamental literal corresponds to one bit, and is considered to
represent `1` if it is any other value than exactly zero.

`bitvec!` can be invoked with no specifiers, a `BitOrder` specifier, or a
`BitOrder` and a `BitStore` specifier. It cannot be invoked with a `BitStore`
specifier but no `BitOrder` specifier, due to overlap in how those tokens are
matched by the macro system.

Like `vec!`, `bitvec!` supports bit lists `[0, 1, …]` and repetition markers
`[1; n]`.

# Examples

```rust
use bitvec::prelude::*;

bitvec![Msb0, u8; 0, 1];
bitvec![Lsb0, u8; 0, 1,];
bitvec![Msb0; 0, 1];
bitvec![Lsb0; 0, 1,];
bitvec![0, 1];
bitvec![0, 1,];
bitvec![Msb0, u8; 1; 5];
bitvec![Lsb0; 0; 5];
bitvec![1; 5];
```
**/
#[macro_export]
#[cfg(feature = "alloc")]
macro_rules! bitvec {
	//  First, capture the repetition syntax, as it is permitted to use runtime
	//  values for the repetition count.
	($order:ty, $store:ident; $val:expr; $rep:expr) => {
		$crate::vec::BitVec::<$order, $store>::repeat($val != 0, $rep)
	};

	($order:ty; $val:expr; $rep:expr) => {
		$crate::vec::BitVec::<$order, usize>::repeat($val != 0, $rep)
	};

	($val:expr; $rep:expr) => {
		$crate::vec::BitVec::<$crate::order::Lsb0, usize>::repeat($val != 0, $rep)
	};

	//  Delegate all others to the `bits!` macro.
	($($arg:tt)*) => {{
		$crate::vec::BitVec::from_bitslice($crate::bits!($($arg)*))
	}};
}

/** Constructs a `BitBox` out of a literal array in source code, like `bitvec!`.

This has exactly the same syntax as [`bitvec!`], and in fact is a thin wrapper
around `bitvec!` that calls `.into_boxed_slice()` on the produced `BitVec` to
freeze it.

[`bitvec!`]: #macro.bitvec
**/
#[macro_export]
#[cfg(feature = "alloc")]
macro_rules! bitbox {
	($($arg:tt)*) => {
		$crate::bitvec!($($arg)*).into_boxed_bitslice()
	};
}

#[cfg(test)]
mod tests {
	#[allow(unused_imports)]
	use crate::order::{
		Lsb0,
		Msb0,
	};

	#[test]
	#[cfg(feature = "alloc")]
	fn compile_bits_macros() {
		bits![0, 1];
		bits![Msb0; 0, 1];
		bits![Lsb0; 0, 1];
		bits![Msb0, u8; 0, 1];
		bits![Lsb0, u8; 0, 1];
		bits![Msb0, u16; 0, 1];
		bits![Lsb0, u16; 0, 1];
		bits![Msb0, u32; 0, 1];
		bits![Lsb0, u32; 0, 1];

		#[cfg(target_pointer_width = "64")]
		{
			bits![Msb0, u64; 0, 1];
			bits![Lsb0, u64; 0, 1];
		}

		bits![1; 70];
		bits![Msb0; 0; 70];
		bits![Lsb0; 1; 70];
		bits![Msb0, u8; 0; 70];
		bits![Lsb0, u8; 1; 70];
		bits![Msb0, u16; 0; 70];
		bits![Lsb0, u16; 1; 70];
		bits![Msb0, u32; 0; 70];
		bits![Lsb0, u32; 1; 70];

		#[cfg(target_pointer_width = "64")]
		{
			bits![Msb0, u64; 0; 70];
			bits![Lsb0, u64; 1; 70];
		}
	}

	#[test]
	#[cfg(feature = "alloc")]
	fn compile_bitvec_macros() {
		bitvec![0, 1];
		bitvec![Msb0; 0, 1];
		bitvec![Lsb0; 0, 1];
		bitvec![Msb0, u8; 0, 1];
		bitvec![Lsb0, u8; 0, 1];
		bitvec![Msb0, u16; 0, 1];
		bitvec![Lsb0, u16; 0, 1];
		bitvec![Msb0, u32; 0, 1];
		bitvec![Lsb0, u32; 0, 1];

		#[cfg(target_pointer_width = "64")]
		{
			bitvec![Msb0, u64; 0, 1];
			bitvec![Lsb0, u64; 0, 1];
		}

		bitvec![1; 70];
		bitvec![Msb0; 0; 70];
		bitvec![Lsb0; 1; 70];
		bitvec![Msb0, u8; 0; 70];
		bitvec![Lsb0, u8; 1; 70];
		bitvec![Msb0, u16; 0; 70];
		bitvec![Lsb0, u16; 1; 70];
		bitvec![Msb0, u32; 0; 70];
		bitvec![Lsb0, u32; 1; 70];

		#[cfg(target_pointer_width = "64")]
		{
			bitvec![Msb0, u64; 0; 70];
			bitvec![Lsb0, u64; 1; 70];
		}
	}

	#[test]
	#[cfg(feature = "alloc")]
	fn compile_bitbox_macros() {
		bitbox![0, 1];
		bitbox![Msb0; 0, 1];
		bitbox![Lsb0; 0, 1];
		bitbox![Msb0, u8; 0, 1];
		bitbox![Lsb0, u8; 0, 1];
		bitbox![Msb0, u16; 0, 1];
		bitbox![Lsb0, u16; 0, 1];
		bitbox![Msb0, u32; 0, 1];
		bitbox![Lsb0, u32; 0, 1];

		#[cfg(target_pointer_width = "64")]
		{
			bitbox![Msb0, u64; 0, 1];
			bitbox![Lsb0, u64; 0, 1];
		}

		bitbox![1; 70];
		bitbox![Msb0; 0; 70];
		bitbox![Lsb0; 1; 70];
		bitbox![Msb0, u8; 0; 70];
		bitbox![Lsb0, u8; 1; 70];
		bitbox![Msb0, u16; 0; 70];
		bitbox![Lsb0, u16; 1; 70];
		bitbox![Msb0, u32; 0; 70];
		bitbox![Lsb0, u32; 1; 70];

		#[cfg(target_pointer_width = "64")]
		{
			bitbox![Msb0, u64; 0; 70];
			bitbox![Lsb0, u64; 1; 70];
		}
	}
}
