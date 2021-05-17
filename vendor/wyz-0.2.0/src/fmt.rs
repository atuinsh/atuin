/*! Format forwarding

This module provides wrapper types for each formatting trait other than `Debug`
which, when `Debug`-formatted, forward to the original trait instead of `Debug`.

Each wrapper type is a tuple struct so that it can be used as a named
constructor, such as in `.map(FmtDisplay)`. In addition, a blanket trait adds
extension methods `.fmt_<trait_name>>()` to provide the corresponding wrap.

Any modifiers in the format template string or struct modifier are passed
through to the desired trait implementation unchanged. The only effect of the
forwarding types in this module is to change the `?` template character to one
of the other trait signifiers.
!*/

use core::{
	fmt::{
		self,
		Binary,
		Debug,
		Display,
		Formatter,
		LowerExp,
		LowerHex,
		Octal,
		Pointer,
		UpperExp,
		UpperHex,
	},
	ops::{
		Deref,
		DerefMut,
	},
};

/// Wraps any value with a format-forward to `Debug`.
pub trait FmtForward: Sized {
	/// Causes `self` to use its `Binary` implementation when `Debug`-formatted.
	fn fmt_binary(self) -> FmtBinary<Self>
	where Self: Binary {
		FmtBinary(self)
	}

	/// Causes `self` to use its `Display` implementation when
	/// `Debug`-formatted.
	fn fmt_display(self) -> FmtDisplay<Self>
	where Self: Display {
		FmtDisplay(self)
	}

	/// Causes `self` to use its `LowerExp` implementation when
	/// `Debug`-formatted.
	fn fmt_lower_exp(self) -> FmtLowerExp<Self>
	where Self: LowerExp {
		FmtLowerExp(self)
	}

	/// Causes `self` to use its `LowerHex` implementation when
	/// `Debug`-formatted.
	fn fmt_lower_hex(self) -> FmtLowerHex<Self>
	where Self: LowerHex {
		FmtLowerHex(self)
	}

	/// Causes `self` to use its `Octal` implementation when `Debug`-formatted.
	fn fmt_octal(self) -> FmtOctal<Self>
	where Self: Octal {
		FmtOctal(self)
	}

	/// Causes `self` to use its `Pointer` implementation when
	/// `Debug`-formatted.
	fn fmt_pointer(self) -> FmtPointer<Self>
	where Self: Pointer {
		FmtPointer(self)
	}

	/// Causes `self` to use its `UpperExp` implementation when
	/// `Debug`-formatted.
	fn fmt_upper_exp(self) -> FmtUpperExp<Self>
	where Self: UpperExp {
		FmtUpperExp(self)
	}

	/// Causes `self` to use its `UpperHex` implementation when
	/// `Debug`-formatted.
	fn fmt_upper_hex(self) -> FmtUpperHex<Self>
	where Self: UpperHex {
		FmtUpperHex(self)
	}
}

impl<T: Sized> FmtForward for T {
}

/// Forwards a type’s `Binary` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtBinary<T: Binary>(pub T);

/// Forwards a type’s `Display` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtDisplay<T: Display>(pub T);

/// Forwards a type’s `LowerExp` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtLowerExp<T: LowerExp>(pub T);

/// Forwards a type’s `LowerHex` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtLowerHex<T: LowerHex>(pub T);

/// Forwards a type’s `Octal` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtOctal<T: Octal>(pub T);

/// Forwards a type’s `Pointer` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtPointer<T: Pointer>(pub T);

/// Forwards a type’s `UpperExp` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtUpperExp<T: UpperExp>(pub T);

/// Forwards a type’s `UpperHex` formatting implementation to `Debug`.
#[repr(transparent)]
pub struct FmtUpperHex<T: UpperHex>(pub T);

macro_rules! fmt {
	($($w:ty => $t:ident),* $(,)?) => { $(
		impl<T: $t + Binary> Binary for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				Binary::fmt(&self.0, fmt)
			}
		}

		impl<T: $t> Debug for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				<T as $t>::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + Display> Display for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				Display::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + LowerExp> LowerExp for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				LowerExp::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + LowerHex> LowerHex for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				LowerHex::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + Octal> Octal for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				Octal::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + Pointer> Pointer for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				Pointer::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + UpperExp> UpperExp for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				UpperExp::fmt(&self.0, fmt)
			}
		}

		impl<T: $t + UpperHex> UpperHex for $w {
			fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
				UpperHex::fmt(&self.0, fmt)
			}
		}

		impl<T: $t> Deref for $w {
			type Target = T;

			fn deref(&self) -> &Self::Target {
				&self.0
			}
		}

		impl<T: $t> DerefMut for $w {
			fn deref_mut(&mut self) -> &mut Self::Target {
				&mut self.0
			}
		}

		impl<T: $t> AsRef<T> for $w {
			fn as_ref(&self) -> &T {
				&self.0
			}
		}

		impl<T: $t> AsMut<T> for $w {
			fn as_mut(&mut self) -> &mut T {
				&mut self.0
			}
		}
	)* };
}

fmt!(
	FmtBinary<T> => Binary,
	FmtDisplay<T> => Display,
	FmtLowerExp<T> => LowerExp,
	FmtLowerHex<T> => LowerHex,
	FmtOctal<T> => Octal,
	FmtPointer<T> => Pointer,
	FmtUpperExp<T> => UpperExp,
	FmtUpperHex<T> => UpperHex,
);
