/*! Internal implementation macros for the public exports.

The macros in this module are required to be exported from the crate, as the
public macros will call them from client contexts (`macro_rules!` expansion
bodies are not in source crate scope, as they are token expansion rather than
symbolic calls). However, they are not part of the public *API* of the crate,
and are not intended for use anywhere but in the expansion bodies of the
public-API constructor macros.
!*/

#![doc(hidden)]

/** Accumulates a stream of bit expressions into a compacted array of elements.

This macro constructs a well-ordered `[T; N]` array expression usable in `const`
contexts. Callers may then use that expression as the source slice over which to
construct `bitvec` types.
**/
#[doc(hidden)]
#[macro_export]
macro_rules! __bits_store_array {
	//  Reroute `usize` to the correct concrete type, and mark the alias.
	//  The `@ usz` causes `as usize` to be appended to exprs as needed.
	($order:tt, usize; $($val:expr),*) => {{
		const LEN: usize = $crate::__count_elts!(usize; $($val),*);

		//  Attributes are not currently allowed on expressions, only items and
		//  statements, so the routing here must bind to a name.
		#[cfg(target_pointer_width = "32")]
		const OUT: [usize; LEN] = $crate::__bits_store_array!(
			$order, u32 @ usz; $($val),*
		);

		#[cfg(target_pointer_width = "64")]
		const OUT: [usize; LEN] = $crate::__bits_store_array!(
			$order, u64 @ usz; $($val),*
		);

		OUT
	}};
	// Entry point.
	($order:tt, $store:ident $(@ $usz:ident )?; $($val:expr),*) => {
		$crate::__bits_store_array!(
			 $order, $store $(@ $usz)?, []; $($val,)*
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 16
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 32
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 48
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0  // 64
		);
	};

	/* NOTE: These have to be first. They (ab)use a quirk of `macro_rules!`
	where `:expr` captures are considered a single `:tt` after being matched.
	Even if the `:expr` matcher was a literal `0`, after being wrapped by the
	`:expr` fragment, it is no longer considered to match a literal `0`, so
	these patterns will only match the extra padding `0`s added at the end.

	Once the user-provided `$val` expressions are all consumed, the remaining
	`0` tokens inserted by the arm above are all removed, ensuring that the
	produced array has no wasted elements.
	*/
	($order:tt, $store:ident @ usz, [$( ($($elt:tt),*) )*]; $(0),*) => {
		[$(
			$crate::__elt_from_bits!($order, $store; $($elt),*) as usize
		),*]
	};
	($order:tt, $store:ident, [$( ($($elt:tt),*) )*]; $(0),*) => {
		[$(
			$crate::__elt_from_bits!($order, $store; $($elt),*)
		),*]
	};

	// Matchers for each size of word. The end of the word may be padded out
	// with `0`s.
	(
		$order:tt, u8 $(@ $usz:ident)?, [$($w:tt)*];
		$a0:tt, $b0:tt, $c0:tt, $d0:tt, $e0:tt, $f0:tt, $g0:tt, $h0:tt
		$(, $($t:tt)*)?
	) => {
		$crate::__bits_store_array!(
			$order, u8 $(@ $usz)?, [$($w)* (
				$a0, $b0, $c0, $d0, $e0, $f0, $g0, $h0
			)];
			$($($t)*)?
		)
	};
	(
		$order:tt, u16 $(@ $usz:ident)?, [$($w:tt)*];
		$a0:tt, $b0:tt, $c0:tt, $d0:tt, $e0:tt, $f0:tt, $g0:tt, $h0:tt,
		$a1:tt, $b1:tt, $c1:tt, $d1:tt, $e1:tt, $f1:tt, $g1:tt, $h1:tt
		$(, $($t:tt)*)?
	) => {
		$crate::__bits_store_array!(
			$order, u16 $(@ $usz)?, [$($w)* (
				$a0, $b0, $c0, $d0, $e0, $f0, $g0, $h0,
				$a1, $b1, $c1, $d1, $e1, $f1, $g1, $h1
			)];
			$($($t)*)?
		)
	};
	(
		$order:tt, u32 $(@ $usz:ident)?, [$($w:tt)*];
		$a0:tt, $b0:tt, $c0:tt, $d0:tt, $e0:tt, $f0:tt, $g0:tt, $h0:tt,
		$a1:tt, $b1:tt, $c1:tt, $d1:tt, $e1:tt, $f1:tt, $g1:tt, $h1:tt,
		$a2:tt, $b2:tt, $c2:tt, $d2:tt, $e2:tt, $f2:tt, $g2:tt, $h2:tt,
		$a3:tt, $b3:tt, $c3:tt, $d3:tt, $e3:tt, $f3:tt, $g3:tt, $h3:tt
		$(, $($t:tt)*)?
	) => {
		$crate::__bits_store_array!(
			$order, u32 $(@ $usz)?, [$($w)* (
				$a0, $b0, $c0, $d0, $e0, $f0, $g0, $h0,
				$a1, $b1, $c1, $d1, $e1, $f1, $g1, $h1,
				$a2, $b2, $c2, $d2, $e2, $f2, $g2, $h2,
				$a3, $b3, $c3, $d3, $e3, $f3, $g3, $h3
			)];
			$($($t)*)?
		)
	};
	(
		$order:tt, u64 $(@ $usz:ident)?, [$($w:tt)*];
		$a0:tt, $b0:tt, $c0:tt, $d0:tt, $e0:tt, $f0:tt, $g0:tt, $h0:tt,
		$a1:tt, $b1:tt, $c1:tt, $d1:tt, $e1:tt, $f1:tt, $g1:tt, $h1:tt,
		$a2:tt, $b2:tt, $c2:tt, $d2:tt, $e2:tt, $f2:tt, $g2:tt, $h2:tt,
		$a3:tt, $b3:tt, $c3:tt, $d3:tt, $e3:tt, $f3:tt, $g3:tt, $h3:tt,
		$a4:tt, $b4:tt, $c4:tt, $d4:tt, $e4:tt, $f4:tt, $g4:tt, $h4:tt,
		$a5:tt, $b5:tt, $c5:tt, $d5:tt, $e5:tt, $f5:tt, $g5:tt, $h5:tt,
		$a6:tt, $b6:tt, $c6:tt, $d6:tt, $e6:tt, $f6:tt, $g6:tt, $h6:tt,
		$a7:tt, $b7:tt, $c7:tt, $d7:tt, $e7:tt, $f7:tt, $g7:tt, $h7:tt
		$(, $($t:tt)*)?
	) => {
		$crate::__bits_store_array!(
			$order, u64 $(@ $usz)?, [$($w)* (
				$a0, $b0, $c0, $d0, $e0, $f0, $g0, $h0,
				$a1, $b1, $c1, $d1, $e1, $f1, $g1, $h1,
				$a2, $b2, $c2, $d2, $e2, $f2, $g2, $h2,
				$a3, $b3, $c3, $d3, $e3, $f3, $g3, $h3,
				$a4, $b4, $c4, $d4, $e4, $f4, $g4, $h4,
				$a5, $b5, $c5, $d5, $e5, $f5, $g5, $h5,
				$a6, $b6, $c6, $d6, $e6, $f6, $g6, $h6,
				$a7, $b7, $c7, $d7, $e7, $f7, $g7, $h7
			)];
			$($($t)*)?
		)
	};
}

/// Counts the number of repetitions inside a `$()*` sequence.
#[doc(hidden)]
#[macro_export]
macro_rules! __count {
	(@ $val:expr) => { 1 };
	($($val:expr),*) => {{
		/* Clippy warns that `.. EXPR + 1`, for any value of `EXPR`, should be
		replaced with `..= EXPR`. This means that `.. $crate::__count!` raises
		the lint, causing `bits![(val,)…]` to have an unfixable lint warning.
		By binding to a `const`, then returning the `const`, this syntax
		construction is avoided as macros only expand to
		`.. { const LEN = …; LEN }` rather than `.. 0 (+ 1)…`.
		*/
		const LEN: usize = 0usize $(+ $crate::__count!(@ $val))*;
		LEN
	}};
}

/// Counts the number of elements needed to store a number of bits.
#[doc(hidden)]
#[macro_export]
macro_rules! __count_elts {
	($t:ident; $($val:expr),*) => {{
		$crate::mem::elts::<$t>($crate::__count!($($val),*))
	}};
}

/// Construct a `T` element from an array of `u8`.
#[doc(hidden)]
#[macro_export]
macro_rules! __elt_from_bits {
	//  Known orderings can be performed immediately.
	(
		Lsb0, $store:ident;
		$(
			$a:expr, $b:expr, $c:expr, $d:expr,
			$e:expr, $f:expr, $g:expr, $h:expr
		),*
	) => {
		$crate::__ty_from_bytes!(
			Lsb0, $store, [$($crate::macros::internal::u8_from_le_bits(
				$a != 0, $b != 0, $c != 0, $d != 0,
				$e != 0, $f != 0, $g != 0, $h != 0,
			)),*]
		)
	};
	(
		Msb0, $store:ident;
		$(
			$a:expr, $b:expr, $c:expr, $d:expr,
			$e:expr, $f:expr, $g:expr, $h:expr
		),*
	) => {
		$crate::__ty_from_bytes!(
			Msb0, $store, [$($crate::macros::internal::u8_from_be_bits(
				$a != 0, $b != 0, $c != 0, $d != 0,
				$e != 0, $f != 0, $g != 0, $h != 0,
			)),*]
		)
	};
	(
		LocalBits, $store:ident;
		$(
			$a:expr, $b:expr, $c:expr, $d:expr,
			$e:expr, $f:expr, $g:expr, $h:expr
		),*
	) => {
		$crate::__ty_from_bytes!(
			LocalBits, $store, [$($crate::macros::internal::u8_from_ne_bits(
				$a != 0, $b != 0, $c != 0, $d != 0,
				$e != 0, $f != 0, $g != 0, $h != 0,
			)),*]
		)
	};

	(
		$order:tt, $store:ident;
		$(
			$a:expr, $b:expr, $c:expr, $d:expr,
			$e:expr, $f:expr, $g:expr, $h:expr
		),*
	) => {{
		let mut tmp: $store = 0;
		let _tmp_bits = BitSlice::<$order, $store>::from_element_mut(&mut tmp);
		let mut _idx = 0;
		$(
			_tmp_bits.set(_idx, $a != 0); _idx += 1;
			_tmp_bits.set(_idx, $b != 0); _idx += 1;
			_tmp_bits.set(_idx, $c != 0); _idx += 1;
			_tmp_bits.set(_idx, $d != 0); _idx += 1;
			_tmp_bits.set(_idx, $e != 0); _idx += 1;
			_tmp_bits.set(_idx, $f != 0); _idx += 1;
			_tmp_bits.set(_idx, $g != 0); _idx += 1;
			_tmp_bits.set(_idx, $h != 0); _idx += 1;
		)*
		tmp
	}};
}

/// Extend a single bit to fill an element.
#[doc(hidden)]
#[macro_export]
macro_rules! __extend_bool {
	($val:expr, $typ:ident) => {
		(0 as $typ).wrapping_sub(($val != 0) as $typ)
	};
}

/// Constructs a fundamental integer from a list of bytes.
#[doc(hidden)]
#[macro_export]
macro_rules! __ty_from_bytes {
	(Msb0, u8, [$($byte:expr),*]) => {
		u8::from_be_bytes([$($byte),*])
	};
	(Lsb0, u8, [$($byte:expr),*]) => {
		u8::from_le_bytes([$($byte),*])
	};
	(LocalBits, u8, [$($byte:expr),*]) => {
		u8::from_ne_bytes([$($byte),*])
	};
	(Msb0, u16, [$($byte:expr),*]) => {
		u16::from_be_bytes([$($byte),*])
	};
	(Lsb0, u16, [$($byte:expr),*]) => {
		u16::from_le_bytes([$($byte),*])
	};
	(LocalBits, u16, [$($byte:expr),*]) => {
		u16::from_ne_bytes([$($byte),*])
	};
	(Msb0, u32, [$($byte:expr),*]) => {
		u32::from_be_bytes([$($byte),*])
	};
	(Lsb0, u32, [$($byte:expr),*]) => {
		u32::from_le_bytes([$($byte),*])
	};
	(LocalBits, u32, [$($byte:expr),*]) => {
		u32::from_ne_bytes([$($byte),*])
	};
	(Msb0, u64, [$($byte:expr),*]) => {
		u64::from_be_bytes([$($byte),*])
	};
	(Lsb0, u64, [$($byte:expr),*]) => {
		u64::from_le_bytes([$($byte),*])
	};
	(LocalBits, u64, [$($byte:expr),*]) => {
		u64::from_ne_bytes([$($byte),*])
	};
	(Msb0, usize, [$($byte:expr),*]) => {
		usize::from_be_bytes([$($byte),*])
	};
	(Lsb0, usize, [$($byte:expr),*]) => {
		usize::from_le_bytes([$($byte),*])
	};
	(LocalBits, usize, [$($byte:expr),*]) => {
		usize::from_ne_bytes([$($byte),*])
	};
}

/// Construct a `u8` from bits applied in Lsb0-order.
#[allow(clippy::many_single_char_names)]
#[allow(clippy::too_many_arguments)]
pub const fn u8_from_le_bits(
	a: bool,
	b: bool,
	c: bool,
	d: bool,
	e: bool,
	f: bool,
	g: bool,
	h: bool,
) -> u8
{
	(a as u8)
		| ((b as u8) << 1)
		| ((c as u8) << 2)
		| ((d as u8) << 3)
		| ((e as u8) << 4)
		| ((f as u8) << 5)
		| ((g as u8) << 6)
		| ((h as u8) << 7)
}

/// Construct a `u8` from bits applied in Msb0-order.
#[allow(clippy::many_single_char_names)]
#[allow(clippy::too_many_arguments)]
pub const fn u8_from_be_bits(
	a: bool,
	b: bool,
	c: bool,
	d: bool,
	e: bool,
	f: bool,
	g: bool,
	h: bool,
) -> u8
{
	(h as u8)
		| ((g as u8) << 1)
		| ((f as u8) << 2)
		| ((e as u8) << 3)
		| ((d as u8) << 4)
		| ((c as u8) << 5)
		| ((b as u8) << 6)
		| ((a as u8) << 7)
}

#[doc(hidden)]
#[cfg(target_endian = "little")]
pub use self::u8_from_le_bits as u8_from_ne_bits;

#[doc(hidden)]
#[cfg(target_endian = "big")]
pub use self::u8_from_be_bits as u8_from_ne_bits;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn byte_assembly() {
		assert_eq!(
			u8_from_le_bits(false, false, true, true, false, true, false, true),
			0b1010_1100
		);

		assert_eq!(
			u8_from_be_bits(false, false, true, true, false, true, false, true),
			0b0011_0101
		);
	}
}
