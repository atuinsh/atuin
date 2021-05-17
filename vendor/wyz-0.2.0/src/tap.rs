/*! Object Tapping

This crate provides traits for transparently inserting operations into a method
chain. All traits take and return the object on which they act by value, and run
a provided function on a borrow of the value.

This allows methods that do not chain (such as mutators with `&mut self -> ()`
signatures) to be chained.

The traits in this crate provide methods that run some function, `Fn(&T)` or
`Fn(&mut T)`, on a value `T` without changing the binding status of that value.

# Value Tapping

The primary trait of this crate is [`Tap`], which provides two methods: [`tap`]
and [`tap_mut`]. These provide immutable or mutable, respectively, borrows of
the tapped value to a user-provided function. The user function must not have a
return value.

This permits using inspector-style (`Fn(&Self)`) or mutator-style
(`Fn(&mut Self)`) functions in a method chain without breaks or reduction of
access to the main value.

Tap methods never change the type of the object on which they are called. The
`mut`-suffixed methods *are* permitted to change the *value* of their object.

# Trait Tapping

Rust does not have subtyping in the object-oriented sense; rather, it uses
traits to indicate relationships between types and bring behavior of an interior
type to the exterior type. This crate provides taps that use the standard
conversion traits in order to assist in running tap methods generically.

## Borrowed Tapping

The traits `std::borrow::Borrow` and `std::borrow::BorrowMut` allow container
types to behave as their contained types in borrowed contexts. The [`TapBorrow`]
trait provides methods, [`tap_borrow`] and [`tap_borrow_mut`], which depend on
`Borrow` and `BorrowMut`, respectively, to run the user-provided function on the
borrowed interior type.

This is useful for inspecting the interior of a `Cow` or other data structures
that abstract away the exact container type but provide uniform access to the
underlying data.

## Polymorphic Tapping

The traits `std::convert::AsRef` and `std::convert::AsMut` allow composed types
to be used by reference as their component types. The [`TapAsRef`] trait
provides methods, [`tap_ref`] and [`tap_ref_mut`], which depend on `AsRef` and
`AsMut`, respectively, to run the user-provided function on the referred
component type.

This is useful for working with types like `Path`, which are commonly used as
generic targets such as `<P: AsRef<Path>>`. All such types `P` may have
`.tap_ref` called upon them with methods implemented on `Path`.

> Note: `Borrow` and `AsRef` are generic traits, which a type can implement many
> times with different targets. As such, the referent type must be specified in
> the tapped function. This can be done with a named method, or by marking the
> type of the closure argument: `|x: &Referent| ...`.

## Dereferenced Tapping

The traits `std::ops::Deref` and `std::ops::DerefMut` may be used to make owning
containers transparently defer to their contained data. This is used by `Vec`
and `String`, for example, to behave like `[T]` and `str` implicitly.

The [`TapDeref`] trait provides [`tap_deref`] and [`tap_deref_mut`] which call
`Deref` or `DerefMut`, respectively, on the tapped value before running the
provided function on the produced `Deref::Target` value.

Since `Deref` may only be implemented once, this trait does not require any
extra type information in its tap calls.

# Conditional Tapping

Additional traits are provided to only invoke the tap when certain conditions
are met in the value being tapped.

## Boolean Tapping

The [`TapBool`] trait, with methods [`tap_true`], [`tap_false`], and their
associated `_mut` variants, run the provided function only when the value is of
the correct variant. This trait is implemented on `bool` by default, and is
left open so that user crates may implement it on their own `bool`-like types.

## Optional Tapping

The [`TapOption`] trait, with methods [`tap_some`], [`tap_some_mut`], and
[`tap_none`], run the provided function only when the `Option` is of the
matching variant. The `tap_some` methods pass `&T` or `&mut T` to their
function; `tap_none` passes nothing.

Note that `tap_some_mut` may change the value of the inner object, but it cannot
change the `Option` from `Some` to `None`. If this behavior is desired, use
`tap_mut` to modify the `Option` wrapper directly, rather than `tap_some_mut` to
change the interior value.

## Result Tapping

This acts exactly like `TapOption`, except that the alternate case has a value
that may be modified. It thus has methods [`tap_ok`], [`tap_err`], and the
associated `_mut` variants.

## Debug Tapping

All methods in the crate have a sibling method with the exact same name and
signature, except that the name is suffixed with `_dbg`. This method runs the
normal tap in a debug build, and is removed in release builds.

```rust
# macro_rules! debug {
#   ( $( $t:tt )* ) => { eprintln!( $( $t )* ) };
# }
use wyz::tap::TapOption;

Some(5i32).tap_some_dbg(|n| debug!("{}", n));
```

This emits a debug trace when the crate is built in debug mode, and does nothing
when the crate is built in release mode.

# Usage

Import the trait or traits you wish to use, with `use wyz::tap::Tap;`, and then
attach `.tap` methods on the end of any expression you want to inspect or
modify. These methods never change the type or binding status of the object to
which they are attached, and can be added or removed without affecting
neighboring code.

# Examples

This uses `tap_mut` to modify a vector using methods that cannot be chained, and
without converting to an iterator and re-collecting.

```rust
use wyz::tap::Tap;

let v = vec![5, 1, 2, 4, 3]
  .tap_mut(|v| v.sort())
  .tap_mut(|v| v.iter_mut().for_each(|e| *e *= 2))
  .tap_mut(|v| v.reverse());
assert_eq!(&v, &[10, 8, 6, 4, 2]);
```

This uses `tap_some` to implement a conditional flag.

```rust
use wyz::tap::TapOption;

let mut flag = false;

let n = None::<i32>.tap_some(|_| flag = true);
assert!(n.is_none());
assert!(!flag);

let n: Option<i32> = Some(1).tap_some(|_| flag = true);
assert_eq!(n.unwrap(), 1);
assert!(flag);
```

And this uses `tap_err` to log errors without suppressing them.

```rust
# use std::fmt::Display;
use wyz::tap::TapResult;

let mut err_ct = 0;

{
 let mut action = |e: &&str| {
  err_ct += 1;
  eprintln!("ERROR: {}", e);
 };

 Ok::<_, &str>("success").tap_err(&mut action);
 Err::<(), _>("failure").tap_err(&mut action);
} // I didn't want to write the closure twice

assert_eq!(err_ct, 1);
//  printed "ERROR: failure"
```

[`Tap`]: trait.Tap.html
[`TapAsRef`]: trait.TapAsRef.html
[`TapBorrow`]: trait.TapBorrow.html
[`TapDeref`]: trait.TapDeref.html
[`TapOption`]: trait.TapOption.html
[`TapResult`]: trait.TapResult.html
[`tap`]: trait.Tap.html#method.tap
[`tap_borrow`]: trait.TapBorrow.html#method.tap_borrow
[`tap_borrow_mut`]: trait.TapBorrow.html#method.tap_borrow_mut
[`tap_deref`]: trait.TapDeref.html#method.tap_deref
[`tap_deref_mut`]: trait.TapDeref.html#method.tap_deref_mut
[`tap_err`]: trait.TapResult.html#method.tap_err
[`tap_mut`]: trait.Tap.html#method.tap_mut
[`tap_none`]: trait.TapOption.html#method.tap_none
[`tap_ok`]: trait.TapResult.html#method.tap_ok
[`tap_ref`]: trait.TapAsRef.html#method.tap_ref
[`tap_ref_mut`]: trait.TapAsRef.html#method.tap_ref_mut
[`tap_some`]: trait.TapOption.html#method.tap_some
[`tap_some_mut`]: trait.TapOption.html#method.tap_some_mut
!*/

use core::{
	borrow::{
		Borrow,
		BorrowMut,
	},
	ops::{
		Deref,
		DerefMut,
	},
};

/** Value Tap

This trait allows any function that takes a borrowed value to be run on a value
directly, without downgrading the binding.

# Examples

Sorting a vector is a quintessential example of operations that break the flow
of handling a value. It cannot be done in the middle of an operation, because it
has the signature `&mut self -> ()`.

```rust
use wyz::tap::Tap;

let v = vec![5, 1, 4, 2, 3]
  .tap_mut(|v| v.sort())
  .tap_mut(|v| v.reverse())
  .tap_mut(|v| v.iter_mut().for_each(|elt| *elt *= 2));
assert_eq!(&v, &[10, 8, 6, 4, 2]);
```

Note that because `sort` and `reverse` are actually methods on `[T: Ord]`, not
on `Vec<T: Ord>`, they cannot be listed by name in the `tap_mut` call. Their
signature is `&mut [T: Ord] -> ()`, but `tap_mut` provides a `&mut Vec<T: Ord>`,
and deref-coercion does not apply to named functions. The [`TapDeref`] trait
allows this to work.

[`TapDeref`]: trait.TapDeref.html
**/
pub trait Tap: Sized {
	/// Provides immutable access for inspection.
	///
	/// This is most useful for inserting passive inspection points into an
	/// expression, such as for logging or counting.
	///
	/// # Examples
	///
	/// This demonstrates the use of `tap` to inspect a value and log it as it
	/// is transformed.
	///
	/// ```rust
	/// use wyz::tap::Tap;
	///
	/// fn make_value() -> i32 { 5 }
	/// fn alter_value(n: i32) -> i32 { n * 3 }
	///
	/// let mut init_flag = false;
	/// let mut fini_flag = false;
	/// let finished = make_value()
	///   .tap(|n| init_flag = *n == 5)
	///   .tap_mut(|n| *n = alter_value(*n))
	///   .tap(|n| fini_flag = *n == 15);
	///
	/// assert!(init_flag);
	/// assert!(fini_flag);
	/// assert_eq!(finished, 15);
	/// ```
	///
	/// This example is somewhat contrived, since `tap` is most useful for
	/// logging values with `eprintln!` or the `log` crate and those are hard to
	/// nicely demonstrate in tests.
	fn tap<F, R>(self, func: F) -> Self
	where
		F: FnOnce(&Self) -> R,
		R: Sized,
	{
		func(&self);
		self
	}

	/// Calls `tap` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_dbg<F, R>(self, func: F) -> Self
	where
		F: FnOnce(&Self) -> R,
		R: Sized,
	{
		#[cfg(debug_assertions)]
		return self.tap(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides mutable access for modification.
	///
	/// This is most useful for transforming mutator methods of the kind
	/// `&mut self -> ()` and making them fit in value chains of `self -> Self`.
	///
	/// # Examples
	///
	/// Append to a string without a `let mut` statement.
	///
	/// ```rust
	/// use wyz::tap::Tap;
	/// let full: String = "Hello".to_owned()
	///   .tap_mut(|s| s.push_str(", reader!"));
	/// assert_eq!(full, "Hello, reader!");
	/// ```
	fn tap_mut<F, R>(mut self, func: F) -> Self
	where
		F: FnOnce(&mut Self) -> R,
		R: Sized,
	{
		func(&mut self);
		self
	}

	/// Calls `tap_mut` in debug builds, and does nothing in release builds.
	#[allow(unused_mut, unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_mut_dbg<F, R>(self, func: F) -> Self
	where
		F: FnOnce(&mut Self) -> R,
		R: Sized,
	{
		#[cfg(debug_assertions)]
		return self.tap_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T: Sized> Tap for T {
}

/** Borrowing Tap

This trait runs the [`Borrow`] or [`BorrowMut`] trait on the caller, and passes
the borrowed output of it to the action. This permits passing methods defined on
a supertype to the tap of a subtype.

Because a type may implement `Borrow` multiple times, the function passed to
`.tap_borrow` must specify its argument type. This can be done by providing the
name of an explicitly typed function, or by typing a closure’s argument.

# Examples

```rust
use wyz::tap::{Tap, TapBorrow};
let v = vec![5, 1, 4, 2, 3]
  .tap_borrow_mut(<[i32]>::sort)
  .tap_mut(|v| v.reverse());
assert_eq!(&v, &[5, 4, 3, 2, 1]);
```

`tap_mut` on a `Vec<T>` cannot call functions defined in `impl [T]`;
`tap_borrow_mut` can, because `Vec<T>` implements `Borrow<[T]>`.

[`Borrow`]: https://doc.rust-lang.org/stable/core/borrow/trait.Borrow.html
[`BorrowMut`]: https://doc.rust-lang.org/stable/core/borrow/trait.BorrowMut.html
**/
pub trait TapBorrow<T: ?Sized>: Sized {
	/// Provides immutable access to the borrow for inspection.
	///
	/// This calls `<Self as Borrow<T>>::borrow` on `self`, and calls `func` on
	/// the resulting borrow.
	///
	/// # Examples
	///
	/// ```rust
	/// use std::rc::Rc;
	/// use wyz::tap::TapBorrow;
	///
	/// let mut len = 0;
	/// let text = Rc::<str>::from("hello")
	///   .tap_borrow(|s: &str| len = s.len());
	/// ```
	fn tap_borrow<F, R>(self, func: F) -> Self
	where
		Self: Borrow<T>,
		F: FnOnce(&T) -> R,
		R: Sized,
	{
		func(Borrow::<T>::borrow(&self));
		self
	}

	/// Calls `tap_borrow` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_borrow_dbg<F, R>(self, func: F) -> Self
	where
		Self: Borrow<T>,
		F: FnOnce(&T) -> R,
		R: Sized,
	{
		#[cfg(debug_assertions)]
		return self.tap_borrow(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides mutable access to the borrow for modification.
	fn tap_borrow_mut<F, R>(mut self, func: F) -> Self
	where
		Self: BorrowMut<T>,
		F: FnOnce(&mut T) -> R,
		R: Sized,
	{
		func(BorrowMut::<T>::borrow_mut(&mut self));
		self
	}

	/// Calls `tap_borrow_mut` in debug builds, and does nothing in release
	/// builds.
	#[allow(unused_mut, unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_borrow_mut_dbg<F, R>(self, func: F) -> Self
	where
		Self: BorrowMut<T>,
		F: FnOnce(&mut T) -> R,
		R: Sized,
	{
		#[cfg(debug_assertions)]
		return self.tap_borrow_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T: Sized, U: ?Sized> TapBorrow<U> for T {
}

/** Referential Tap

This trait runs the [`AsRef`] or [`AsMut`] trait on the caller, and passes the
referred output of it to the action. This permits passing methods defined on a
member value’s type to the tap of an aggregate value.

Due to restrictions in the Rust type system, using these taps on types which
have multiple implementations of `AsRef` or `AsMut` must specify which
implementation is desired by setting the type of the receiver of the called
function.

# Examples

```rust
use wyz::tap::{Tap, TapAsRef};
let v = vec![5, 1, 4, 2, 3]
  .tap_ref_mut(<[i32]>::sort)
  .tap_mut(|v| v.reverse());
assert_eq!(&v, &[5, 4, 3, 2, 1]);
```

This example demonstrates disambiguating among multiple implementations.

```rust
use wyz::tap::TapAsRef;

struct Example {
 a: [u8; 8],
 b: [u16; 4],
}
impl AsRef<[u8]> for Example {
 fn as_ref(&self) -> &[u8] {
  &self.a
 }
}
impl AsRef<[u16]> for Example {
 fn as_ref(&self) -> &[u16] {
  &self.b
 }
}
impl AsMut<[u8]> for Example {
 fn as_mut(&mut self) -> &mut [u8] {
  &mut self.a
 }
}
impl AsMut<[u16]> for Example {
 fn as_mut(&mut self) -> &mut [u16] {
  &mut self.b
 }
}

let mut sum = 0usize;
let e = Example {
 a: [0, 1, 2, 3, 4, 5, 6, 7],
 b: [8, 9, 10, 11],
}
 .tap_ref(|a: &[u8]| sum += a.iter().sum::<u8>() as usize)
 .tap_ref(|b: &[u16]| sum += b.iter().sum::<u16>() as usize)
 .tap_ref_mut(|a: &mut [u8]| a.iter_mut().for_each(|e| *e *= 2))
 .tap_ref_mut(|b: &mut [u16]| b.iter_mut().for_each(|e| *e *= 2));

assert_eq!(sum, 66);
assert_eq!(e.a, [0, 2, 4, 6, 8, 10, 12, 14]);
assert_eq!(e.b, [16, 18, 20, 22]);
```

[`AsMut`]: https://doc.rust-lang.org/stable/core/convert/trait.AsMut.html
[`AsRef`]: https://doc.rust-lang.org/stable/core/convert/trait.AsRef.html
**/
pub trait TapAsRef<T: ?Sized>: Sized {
	/// Provides immutable access to the reference for inspection.
	fn tap_ref<F, R>(self, func: F) -> Self
	where
		Self: AsRef<T>,
		F: FnOnce(&T) -> R,
	{
		func(AsRef::<T>::as_ref(&self));
		self
	}

	/// Calls `tap_ref` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_ref_dbg<F, R>(self, func: F) -> Self
	where
		Self: AsRef<T>,
		F: FnOnce(&T) -> R,
	{
		#[cfg(debug_assertions)]
		return self.tap_ref(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides mutable access to the reference for modification.
	fn tap_ref_mut<F, R>(mut self, func: F) -> Self
	where
		Self: AsMut<T>,
		F: FnOnce(&mut T) -> R,
	{
		func(AsMut::<T>::as_mut(&mut self));
		self
	}

	/// Calls `tap_ref_mut` in debug builds, and does nothing in release builds.
	#[allow(unused_mut, unused_variables)]
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_ref_mut_dbg<F, R>(mut self, func: F) -> Self
	where
		Self: AsMut<T>,
		F: FnOnce(&mut T) -> R,
	{
		#[cfg(debug_assertions)]
		return self.tap_ref_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T: Sized, U: ?Sized> TapAsRef<U> for T {
}

/** Dereferencing Tap

This trait runs the [`Deref`] or [`DerefMut`] trait on the caller, and passes
the reborrowed dereference of it to the action. This permits passing methods
defined on the supertype to the tap of a subtype *by name*, rather than by using
closure syntax.

Note that the implementation of this trait does not require that the implementor
also implement `Deref` or `DerefMut`, but the trait methods will cause compiler
failures at the call site if the `Deref` or `DerefMut` traits are not present.

# Examples

```rust
use wyz::tap::{Tap, TapDeref};
let v = vec![5, 1, 4, 2, 3]
  .tap_deref_mut(<[i32]>::sort)
  .tap_mut(|v| v.reverse());
assert_eq!(&v, &[5, 4, 3, 2, 1]);
```

[`Deref`]: https://doc.rust-lang.org/stable/core/ops/trait.Deref.html
[`DerefMut`]: https://doc.rust-lang.org/stable/core/ops/trait.DerefMut.html
**/
pub trait TapDeref: Sized {
	/// Immutably dereferences `self` for inspection.
	fn tap_deref<F, R>(self, func: F) -> Self
	where
		Self: Deref,
		F: FnOnce(&<Self as Deref>::Target) -> R,
	{
		func(Deref::deref(&self));
		self
	}

	/// Calls `tap_deref` in debug builds, and does nothing in release builds.
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_deref_dbg<F, R>(self, func: F) -> Self
	where
		Self: Deref,
		F: FnOnce(&<Self as Deref>::Target) -> R,
	{
		#[cfg(debug_assertions)]
		return self.tap_deref(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Mutably dereferences `self` for modification.
	fn tap_deref_mut<F, R>(mut self, func: F) -> Self
	where
		Self: DerefMut,
		F: FnOnce(&mut <Self as Deref>::Target) -> R,
	{
		func(DerefMut::deref_mut(&mut self));
		self
	}

	/// Calls `tap_deref_mut` in debug builds, and does nothing in release
	/// builds.
	#[cfg_attr(not(debug_assertions), allow(unused_variables))]
	fn tap_deref_mut_dbg<F, R>(self, func: F) -> Self
	where
		Self: DerefMut,
		F: FnOnce(&mut <Self as Deref>::Target) -> R,
	{
		#[cfg(debug_assertions)]
		return self.tap_deref_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T: Sized> TapDeref for T {
}

/** Optional Tap

This trait allows conditional tapping of `Option` wrappers. The methods only
invoke their provided function, on the inner type, if the `Option` has the
correct outer variant.
**/
pub trait TapOption<T: Sized>: Sized {
	/// Provides the interior value for inspection if present.
	///
	/// This is equivalent to `.map(|v| { func(&v); v })`.
	fn tap_some<F: FnOnce(&T) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_some` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_some_dbg<F: FnOnce(&T) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_some(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides the interior value for modification if present.
	///
	/// This is equivalent to `.map(|mut v| { func(&mut v); v })`.
	fn tap_some_mut<F: FnOnce(&mut T) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_some_mut` in debug builds, and does nothing in release
	/// builds.
	#[allow(unused_variables)]
	fn tap_some_mut_dbg<F: FnOnce(&mut T) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_some_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Runs the provided function if the `Option` is empty.
	///
	/// This is equivalent to `.or_else(|| { func(); None })`.
	fn tap_none<F: FnOnce() -> R, R>(self, func: F) -> Self;

	/// Calls `tap_none` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_none_dbg<F: FnOnce() -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_none(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T> TapOption<T> for Option<T> {
	fn tap_some<F: FnOnce(&T) -> R, R>(self, func: F) -> Self {
		if let Some(val) = self.as_ref() {
			func(val);
		}
		self
	}

	fn tap_some_mut<F: FnOnce(&mut T) -> R, R>(mut self, func: F) -> Self {
		if let Some(val) = self.as_mut() {
			func(val);
		}
		self
	}

	fn tap_none<F: FnOnce() -> R, R>(self, func: F) -> Self {
		if self.is_none() {
			func();
		}
		self
	}
}

/** Result Tap

This trait allows conditional tapping of `Result` wrappers. The methods only
invoke their provided function, on the inner type, if the `Result` has the
correct outer variant.

Note that the result value of whichever function you pass to the tapper is
discarded, so if that function returns a `Result`, an “unused must use” warning
will be raised! You must explicitly handle or drop a `Result` value if
your tapper’s function produces one.
**/
pub trait TapResult<T: Sized, E: Sized>: Sized {
	/// Provides the inner value for inspection if the `Result` is `Ok`.
	fn tap_ok<F: FnOnce(&T) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_ok` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_ok_dbg<F: FnOnce(&T) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_ok(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides the inner value for modification if the `Result` is `Ok`.
	fn tap_ok_mut<F: FnOnce(&mut T) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_ok_mut` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_ok_mut_dbg<F: FnOnce(&mut T) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_ok_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides the inner error value for inspection if the `Result` is `Err`.
	fn tap_err<F: FnOnce(&E) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_err` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_err_dbg<F: FnOnce(&E) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_err(func);
		#[cfg(not(debug_assertions))]
		return self;
	}

	/// Provides the inner error value for modification if the `Result` is
	/// `Err`.
	fn tap_err_mut<F: FnOnce(&mut E) -> R, R>(self, func: F) -> Self;

	/// Calls `tap_err_mut` in debug builds, and does nothing in release builds.
	#[allow(unused_variables)]
	fn tap_err_mut_dbg<F: FnOnce(&mut E) -> R, R>(self, func: F) -> Self {
		#[cfg(debug_assertions)]
		return self.tap_err_mut(func);
		#[cfg(not(debug_assertions))]
		return self;
	}
}

impl<T, E> TapResult<T, E> for Result<T, E> {
	fn tap_ok<F: FnOnce(&T) -> R, R>(self, func: F) -> Self {
		if let Ok(val) = self.as_ref() {
			func(val);
		}
		self
	}

	fn tap_ok_mut<F: FnOnce(&mut T) -> R, R>(mut self, func: F) -> Self {
		if let Ok(val) = self.as_mut() {
			func(val);
		}
		self
	}

	fn tap_err<F: FnOnce(&E) -> R, R>(self, func: F) -> Self {
		if let Err(err) = self.as_ref() {
			func(err);
		}
		self
	}

	fn tap_err_mut<F: FnOnce(&mut E) -> R, R>(mut self, func: F) -> Self {
		if let Err(err) = self.as_mut() {
			func(err);
		}
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tap() {
		let mut trap = 0;
		assert_eq!(5.tap(|n| trap += *n), 5);
		assert_eq!(5.tap_mut(|n| *n += trap), 10);
	}

	#[cfg(feature = "alloc")]
	#[test]
	fn tap_borrow() {
		use alloc::rc::Rc;

		let mut len = 0;
		let _ = Rc::<str>::from("hello").tap_borrow(|s: &str| len = s.len());
		assert_eq!(len, 5);

		let v = alloc::vec![5i32, 1, 2, 4, 3]
			.tap_borrow_mut(<[i32]>::sort)
			.tap_borrow_mut(<[i32]>::reverse);
		assert_eq!(v, &[5, 4, 3, 2, 1])
	}

	#[test]
	fn tap_as_ref() {
		struct Example {
			a: [u8; 8],
			b: [u16; 4],
		}
		impl AsRef<[u8]> for Example {
			fn as_ref(&self) -> &[u8] {
				&self.a
			}
		}
		impl AsRef<[u16]> for Example {
			fn as_ref(&self) -> &[u16] {
				&self.b
			}
		}
		impl AsMut<[u8]> for Example {
			fn as_mut(&mut self) -> &mut [u8] {
				&mut self.a
			}
		}
		impl AsMut<[u16]> for Example {
			fn as_mut(&mut self) -> &mut [u16] {
				&mut self.b
			}
		}

		let mut sum_a = 0;
		let mut sum_b = 0;
		let e = Example {
			a: [0, 1, 2, 3, 4, 5, 6, 7],
			b: [8, 9, 10, 11],
		}
		.tap_ref(|a: &[u8]| sum_a = a.iter().sum())
		.tap_ref(|b: &[u16]| sum_b = b.iter().sum())
		.tap_ref_mut(|a: &mut [u8]| a.iter_mut().for_each(|e| *e *= 2))
		.tap_ref_mut(|b: &mut [u16]| b.iter_mut().for_each(|e| *e *= 2));

		assert_eq!(sum_a, 28);
		assert_eq!(sum_b, 38);
		assert_eq!(e.a, [0, 2, 4, 6, 8, 10, 12, 14]);
		assert_eq!(e.b, [16, 18, 20, 22]);
	}

	#[cfg(feature = "alloc")]
	#[test]
	fn tap_deref() {
		let mut len = 0;
		let v = alloc::vec![5, 1, 4, 2, 3]
			.tap_deref(|s| len = s.len())
			.tap_deref_mut(<[i32]>::sort);
		assert_eq!(len, 5);
		assert_eq!(v, [1, 2, 3, 4, 5]);
	}

	#[test]
	fn tap_option() {
		let mut trap = 0;

		None.tap_some(|x: &i32| trap += *x);
		assert_eq!(trap, 0);

		Some(5).tap_some(|x| trap += *x);
		assert_eq!(trap, 5);

		assert_eq!(Some(5).tap_some_mut(|x| *x += 5).unwrap(), 10);

		assert!(Some(5).tap_mut(Option::take).is_none());
	}

	#[test]
	fn tap_result() {
		let mut trap = 0;

		assert_eq!(Err(5).tap_ok(|x: &i32| trap += *x).unwrap_err(), 5);
		assert_eq!(trap, 0);
		assert_eq!(Err(5).tap_ok_mut(|x: &mut i32| *x += 5).unwrap_err(), 5);

		assert_eq!(Ok::<_, i32>(5).tap_ok(|x: &i32| trap += *x).unwrap(), 5);
		assert_eq!(trap, 5);
		assert_eq!(Ok::<_, i32>(5).tap_ok_mut(|x| *x += 5).unwrap(), 10);
	}
}
