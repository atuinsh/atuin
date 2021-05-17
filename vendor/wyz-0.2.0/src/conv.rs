/*! Directed Type Conversion

This module provides sibling traits to the `std::convert` module. The standard
library puts the type parameter in the trait declaration, which makes those
traits generic and suitable for constraint clauses and function calls, but not
usable in indeterminate method-call positions. These traits put the type
parameter in the function declaration, making the trait non-generic and allowing
the function to be called in method-call position without ambiguity.
!*/

use core::convert::TryInto;

/** Directed Type Conversion

This trait is an accessory to [`From`] and [`Into`]. It works by moving the
destination type from the trait name (`Into<Target>::into`) into the method name
(`Conv::conv::<Target>`). This change makes `Into<_>` the correct trait to use
in trait bounds and `.conv::<_>` the correct method to use in expressions.

A `conv::<T>` method is automatically available whenever an `Into<T>`
implementation exists for a type. `Into<T>` is most commonly implemented by
taking advantage of the reflexive blanket implentation using `From`, but can
also be manually implemented as desired.

`.into()` cannot be used in intermediate expressions, because it is impossible
for the compiler’s type engine to select a unique `Into<T>` implementation. This
means that expressions like `v.into().use()` will never compile. Users can
replace `.into()` with `.conv::<Dest>()` in order to inform the compiler of the
type of the expression after the conversion, and make compilation succeed.

`Conv` cannot be used in trait bounds, because the trait itself is not generic.
All `Sized` types implement `Conv` by default, so specifying that a type must be
`Conv` adds no information to the solver.

# Examples

## Conversion as methods

Conversion with `.into()` will fail to compile, even with the type annotation:

```rust,compile_fail
let s: String = "static".into().clone();
//              ^^^^^^^^^^^^^^^ cannot infer type for `T`
// note: type must be known at this point
```

while the equivalent code with `.conv::<_>` does compile:

```rust
# use wyz::conv::Conv;
let s = "static".conv::<String>().clone();
```

## Conversion as traits

Bounding a type with `Conv` will not compile, because the trait itself gives no
information:

```rust,compile_fail
# use wyz::conv::Conv;
fn lift<T: Conv>(src: T) -> String {
  src.conv::<String>().clone()
//    ^^^^ the trait `From<T>` is not implemented for `String`
// help: consider adding a `where String: From<T>` bound
// note: required because of the requirements on the impl of `Into<String>` for `T`
}
```

This can be fixed by adding the clause `where String: From<T>`, or by using the
bound `Into`:

```rust
# use wyz::conv::Conv;
fn lift<T: Into<String>>(src: T) -> String {
  src.conv::<String>().clone()
}
```

The `Into<T>` trait bound makes available both the `Into::<T>::into` method and
the `Conv::conv::<T>` method.

[`From`]: https://doc.rust-lang.org/std/convert/trait.From.html
[`Into`]: https://doc.rust-lang.org/std/convert/trait.Into.html
**/
pub trait Conv: Sized {
	/// Converts `self` into a target type.
	///
	/// This method runs `<Self as Into<T>>::into` on `self` to produce the
	/// desired output. The only difference between using `Conv::conv` and
	/// `Into::into` is where the target type is placed in the name; `.conv()`
	/// can be used in intermediate positions of an expression, while `.into()`
	/// cannot.
	///
	/// # Examples
	///
	/// ```rust
	/// use wyz::conv::Conv;
	///
	/// let t = "hello".conv::<String>();
	/// ```
	fn conv<T: Sized>(self) -> T
	where Self: Into<T> {
		<Self as Into<T>>::into(self)
	}
}

impl<T: Sized> Conv for T {
}

/** Directed Fallible Type Conversion

This trait is an accessory to [`TryFrom`] and [`TryInto`]. It works by moving
the destination type from the trait name (`TryInto<Target>::try_into`) into the
method name (`TryConv::try_conv::<Target>`). This change makes `TryInto<_>` the
correct trait to use in trait bounds and `.try_conv::<_>` the correct method to
use in expressions.

A `try_conv::<T>` method is automatically available whenever a `TryInto<T>`
implementation exists for a type. `TryInto<T>` is most commonly implemented by
taking advantage of the reflexive blanket implentation using `TryFrom`, but can
also be manually implemented as desired.

`.try_into()` cannot be used in intermediate expressions, because it is
impossible for the compiler’s type engine to select a unique `TryInto<T>`
implementation. This means that expressions like `v.try_into().use()` will never
compile. Users can replace `.try_into()` with `.try_conv::<Dest>()` in order to
inform the compiler of the type of the expression after the conversion, and make
compilation succeed.

`TryConv` cannot be used in trait bounds, because the trait itself is not
generic. All `Sized` types implement `TryConv` by default, so specifying that a
type must be `TryConv` adds no information to the solver.

# Examples

## Conversion as methods

Conversion with `.try_into()` will fail to compile, even with the type
annotation:

```rust,ignore
let s: String = "static".try_into().unwrap().clone();
//              ^^^^^^^^^^^^^^^^^^^ cannot infer type for `T`
// note: type must be known at this point
```

while the equivalent code with `.try_conv::<_>` does compile:

```rust
# use wyz::conv::TryConv;
let s = "static".try_conv::<String>().unwrap().clone();
```

## Conversion as traits

Bounding a type with `TryConv` will not compile, because the trait itself gives
no information:

```rust,ignore
# use wyz::conv::TryConv;
fn lift<T: TryConv>(src: T) -> String {
  src.try_conv::<String>().clone()
//    ^^^^^^^^ the trait `From<T>` is not implemented for `String`
// help: consider adding a `where String: From<T>` bound
// note: required because of the requirements on the impl of `Into<String>` for `T`
// note: required because of the requirements on the impl of `TryFrom<T>` for `String`
}
```

This can be fixed by adding the clause `where String: TryFrom<T>`, or by using
the bound `TryInto`:

```rust
# use std::convert::TryInto;
# use wyz::conv::TryConv;
fn lift<T: TryInto<String>>(src: T) -> String {
  src.try_conv::<String>().ok().unwrap().clone()
}
```

The `TryInto<T>` trait bound makes available both the `TryInto::<T>::try_into`
method and the `TryConv::try_conv::<T>` method.

[`TryFrom`]: https://doc.rust-lang.org/std/convert/trait.TryFrom.html
[`TryInto`]: https://doc.rust-lang.org/std/convert/trait.TryInto.html
**/
pub trait TryConv: Sized {
	/// Attempts to convert `self` into a target type.
	///
	/// This method runs `<Self as TryInto<T>>::try_into` on `self` to produce
	/// the desired output. The only difference between using
	/// `TryConv::try_conv` and `TryInto::try_into` is where the target type is
	/// placed in the name; `.try_conv()` can be used in intermediate positions
	/// of an expression, while `.try_into()` cannot.
	///
	/// # Examples
	///
	/// ```rust
	/// use wyz::conv::TryConv;
	///
	/// let t = "hello".try_conv::<String>().unwrap();
	/// ```
	fn try_conv<T: Sized>(self) -> Result<T, Self::Error>
	where Self: TryInto<T> {
		<Self as TryInto<T>>::try_into(self)
	}
}

impl<T: Sized> TryConv for T {
}
