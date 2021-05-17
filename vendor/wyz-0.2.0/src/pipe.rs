/*! Pipe objects into functions, even those not available for dot-call.

Rust restricts the `.method()` call syntax to be available only on functions
defined in `impl [Trait for] Type` blocks, with language-blessed `self`
receivers.

This module allows any function to be `.call()`ed, with as little overhead as
is possible in the language.

# Examples

```rust
use wyz::pipe::*;

fn double(x: i32) -> i32 {
  x * 2
}

fn double_ref(x: &i32) -> i32 {
  *x * 2
}

assert_eq!(5.pipe(double), 10);
assert_eq!(5.pipe_ref(double_ref), 10);
```

Rust’s automatic de/reference chasing only works for the signatures Rust already
permits in method-call syntax; the `Pipe` trait provides methods for certain
reference conversion chasing, but cannot otherwise take advantage of the
language builtin support for such behavior on ordinary methods.

Petition for a `|>` operator; sorry.
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

/** Permit suffixed call of any function on a value.

This trait provides a verbose, ugly version of the functional-language operator
`|>`. For any value, calling `.pipe(some_function)` translates to
`some_function(value)`.

Because it takes `self` by value, this trait is only implemented on `Sized`
types.
**/
pub trait Pipe: Sized {
	/// Pipes a value into a function that cannot ordinarily be called in suffix
	/// position.
	///
	/// # Parameters
	///
	/// - `self`: Any value
	/// - `func`: Any function, which will receive `self` as its first and only
	///   parameter. The return value of this function is then returned from
	///   `pipe`.
	///
	/// Because this is a library function, not a language feature, it is not
	/// able to use the method-call shorthand of `function(...other_params)`. A
	/// suffix function with other params must be called as a closure:
	///
	/// ```rust
	/// use wyz::pipe::Pipe;
	///
	/// fn add(a: i32, b: i32) -> i32 { a + b }
	///
	/// assert_eq!(5.pipe(|a| add(a, 2)), 7);
	/// ```
	///
	/// This is *more* verbose than calling the function in prefix position; its
	/// only value is for fitting into suffix-call chains.
	///
	/// The `.p` method is a shorthand alias for `.pipe`.
	///
	/// # Type Parameters
	///
	/// - `R`: The return value of `func`, which is then returned from `.pipe`.
	///   This is placed as a function type parameter rather than a trait type
	///   parameter so that it can be specified in ambiguous call sites.
	#[inline(always)]
	fn pipe<R>(self, func: impl FnOnce(Self) -> R) -> R
	where R: Sized {
		func(self)
	}
}

/** Referential piping.

The `Pipe` trait passes by value; the functions in this trait pass by reference.
As such, this trait is implemented on all types, not just `Sized`. The methods
in this trait operate by various mechanisms, including:

- normal `&`/`&mut` borrows
- the `Borrow` and `BorrowMut` traits
- the `AsRef` and `AsMut` traits
- the `Deref` and `DerefMut` traits
**/
pub trait PipeRef {
	/// Pipes a reference into a function that cannot ordinarily be called in
	/// suffix position.
	///
	/// # Parameters
	///
	/// - `&self`: A reference to any value. `.pipe_ref` takes it by reference
	///   so that the `.pipe_ref` call will not artificially truncate the
	///   lifetime of `self`.
	/// - `func`: Any function, which receives `&self` as its first and only
	///   parameter. This function may return any value, including a borrow of
	///   `self`, as long as it has an equal or greater lifetime than `self`.
	///
	/// # Type Parameters
	///
	/// - `R`: The return value of `func`. This must have a lifetime of at least
	///   `'a`, up to and including `'static` and unconstrained (borrow-less
	///   value).
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_ref` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_ref<'a, R>(&'a self, func: impl FnOnce(&'a Self) -> R) -> R
	where R: 'a + Sized {
		func(self)
	}

	/// Pipes a mutable reference into a function that cannot ordinarily be
	/// called in suffix position.
	///
	/// # Parameters
	///
	/// - `&mut self`: A mutable reference to any value. `.pipe_mut` takes it by
	///   reference so that the `.pipe_mut` call will not artificially truncate
	///   the lifetime of `self`.
	/// - `func`: Any function, which receives `&mut self` as its first and only
	///   parameter. This funtion may return any value, including a borrow of
	///   `self`, as long as it has an equal or greater lifetime than `self`.
	///
	/// # Type Parameters
	///
	/// - `R`: The return value of `func`. This must have a lifetime of at least
	///   `'a`, up to and including `'static` and unconstrained (borrow-less
	///   value).
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_mut<'a, R>(&'a mut self, func: impl FnOnce(&'a mut Self) -> R) -> R
	where R: 'a + Sized {
		func(self)
	}
}

/// Calls the `Borrow` or `BorrowMut` traits before piping.
pub trait PipeBorrow {
	/// Pipes a trait borrow into a function that cannot normally be called in
	/// suffix position.
	///
	/// # Parameters
	///
	/// - `&self`: A borrow of the receiver. By automatically borrowing, this
	///   pipe call ensures that the lifetime of the origin object is not
	///   artificially truncated.
	/// - `func`: A function which receives a trait-directed borrow of the
	///   receiver’s value type. Before this function is called, `<Self as
	///   Borrow<T>::borrow>` is called on the `self` reference, and the output
	///   of that trait borrow is passed to `func`.
	///
	/// # Type Parameters
	///
	/// - `T`: The type to which `self` `Borrow`s. This is required to be
	///   defined as a method type parameter so that you can disambiguate at
	///   call time.
	/// - `R`: The return type of `func`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_borrow<'a, T, R>(&'a self, func: impl FnOnce(&'a T) -> R) -> R
	where
		Self: Borrow<T>,
		T: 'a,
		R: 'a + Sized,
	{
		func(Borrow::<T>::borrow(self))
	}

	/// Pipes a trait mutable borrow into a function that cannot normally be
	/// called in suffix position.
	///
	/// # Parameters
	///
	/// - `&mut self`: A mutable borrow of the receiver. By automatically
	///   borrowing, this pipe call ensures that the lifetime of the origin
	///   object is not artificially truncated.
	/// - `func`: A function which receives a trait-directed borrow of the
	///   receiver’s value type. Before this function is called, `<Self as
	///   BorrowMut<T>::borrow_mut>` is called on the `self` reference, and the
	///   output of that trait borrow is passed to `func`.
	///
	/// # Type Parameters
	///
	/// - `T`: The type to which `self` borrows. This is required to be defined
	///   as a method type parameter so that you can disambiguate at call time.
	/// - `R`: The return type of `func`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_borrow_mut<'a, T, R>(
		&'a mut self,
		func: impl FnOnce(&'a mut T) -> R,
	) -> R
	where
		Self: BorrowMut<T>,
		T: 'a,
		R: 'a + Sized,
	{
		func(BorrowMut::<T>::borrow_mut(self))
	}
}

/// Calls the `AsRef` or `AsMut` traits before piping.
pub trait PipeAsRef {
	/// Pipes a trait borrow into a function that cannot normally be called in
	/// suffix position.
	///
	/// # Parameters
	///
	/// - `&self`: This borrows `self` for the same reasons as described in the
	///   other methods.
	/// - `func`: A function as described in the other methods. This receives
	///   the result of `AsRef::<T>::as_ref`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_as_ref<'a, T, R>(&'a self, func: impl FnOnce(&'a T) -> R) -> R
	where
		Self: AsRef<T>,
		T: 'a,
		R: 'a + Sized,
	{
		func(AsRef::<T>::as_ref(self))
	}

	/// Pipes a trait mutable borrow into a function that cannot normally be
	/// called in suffix position.
	///
	/// # Parameters
	///
	/// - `&mut self`: This borrows `self` for the same reasons as described in
	///   the other methods.
	/// - `func`: A function as described in the other methods. This receives
	///   the result of `AsMut::<T>::as_mut`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_as_mut<'a, T, R>(
		&'a mut self,
		func: impl FnOnce(&'a mut T) -> R,
	) -> R
	where
		Self: AsMut<T>,
		T: 'a,
		R: 'a + Sized,
	{
		func(AsMut::<T>::as_mut(self))
	}
}

/// Calls the `Deref` or `DerefMut` traits before piping.
pub trait PipeDeref {
	/// Pipes a dereference into a function that cannot normally be called in
	/// suffix position.
	///
	/// # Parameters
	///
	/// - `&self`: This borrows `self` for the same reasons as described in the
	/// - `func`: A function as described in the other methods. This receives
	///   the result of `Deref::deref`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_deref<'a, R>(
		&'a self,
		func: impl FnOnce(&'a <Self as Deref>::Target) -> R,
	) -> R
	where
		Self: Deref,
		R: 'a + Sized,
	{
		func(Deref::deref(self))
	}

	/// Pipes a mutable dereference into a function that cannot normally be
	/// called in suffix position.
	///
	/// # Parameters
	///
	/// - `&mut self`: This mutably borrows `self` for the same reasons as
	///   described in the other methods.
	/// - `func`: A function as described in the other methods. This receives
	///   the result of `DerefMut::deref`.
	///
	/// # Lifetimes
	///
	/// - `'a`: The lifetime of the `self` value. `.pipe_mut` borrows `self` for
	///   the duration `'a`, and extends it through the return value of `func`.
	#[inline(always)]
	fn pipe_deref_mut<'a, R>(
		&'a mut self,
		func: impl FnOnce(&'a mut <Self as Deref>::Target) -> R,
	) -> R
	where
		Self: DerefMut,
		R: 'a + Sized,
	{
		func(DerefMut::deref_mut(self))
	}
}

impl<T: Sized> Pipe for T {
}

impl<T> PipeRef for T {
}

impl<T> PipeBorrow for T {
}

impl<T> PipeAsRef for T {
}

impl<T> PipeDeref for T {
}
