//! The enum [`Either`] with variants `Left` and `Right` is a general purpose
//! sum type with two cases.
//!
//! [`Either`]: enum.Either.html
//!
//! **Crate features:**
//!
//! * `"use_std"`
//! Enabled by default. Disable to make the library `#![no_std]`.
//!
//! * `"serde"`
//! Disabled by default. Enable to `#[derive(Serialize, Deserialize)]` for `Either`
//!

#![doc(html_root_url = "https://docs.rs/either/1/")]
#![cfg_attr(all(not(test), not(feature = "use_std")), no_std)]
#[cfg(all(not(test), not(feature = "use_std")))]
extern crate core as std;

#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;

#[cfg(feature = "serde")]
pub mod serde_untagged;

#[cfg(feature = "serde")]
pub mod serde_untagged_optional;

use std::convert::{AsMut, AsRef};
use std::fmt;
use std::iter;
use std::ops::Deref;
use std::ops::DerefMut;

#[cfg(any(test, feature = "use_std"))]
use std::error::Error;
#[cfg(any(test, feature = "use_std"))]
use std::io::{self, BufRead, Read, Write};

pub use Either::{Left, Right};

/// The enum `Either` with variants `Left` and `Right` is a general purpose
/// sum type with two cases.
///
/// The `Either` type is symmetric and treats its variants the same way, without
/// preference.
/// (For representing success or error, use the regular `Result` enum instead.)
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Either<L, R> {
    /// A value of type `L`.
    Left(L),
    /// A value of type `R`.
    Right(R),
}

macro_rules! either {
    ($value:expr, $pattern:pat => $result:expr) => {
        match $value {
            Either::Left($pattern) => $result,
            Either::Right($pattern) => $result,
        }
    };
}

/// Macro for unwrapping the left side of an `Either`, which fails early
/// with the opposite side. Can only be used in functions that return
/// `Either` because of the early return of `Right` that it provides.
///
/// See also `try_right!` for its dual, which applies the same just to the
/// right side.
///
/// # Example
///
/// ```
/// #[macro_use] extern crate either;
/// use either::{Either, Left, Right};
///
/// fn twice(wrapper: Either<u32, &str>) -> Either<u32, &str> {
///     let value = try_left!(wrapper);
///     Left(value * 2)
/// }
///
/// fn main() {
///     assert_eq!(twice(Left(2)), Left(4));
///     assert_eq!(twice(Right("ups")), Right("ups"));
/// }
/// ```
#[macro_export]
macro_rules! try_left {
    ($expr:expr) => {
        match $expr {
            $crate::Left(val) => val,
            $crate::Right(err) => return $crate::Right(::std::convert::From::from(err)),
        }
    };
}

/// Dual to `try_left!`, see its documentation for more information.
#[macro_export]
macro_rules! try_right {
    ($expr:expr) => {
        match $expr {
            $crate::Left(err) => return $crate::Left(::std::convert::From::from(err)),
            $crate::Right(val) => val,
        }
    };
}

impl<L, R> Either<L, R> {
    /// Return true if the value is the `Left` variant.
    ///
    /// ```
    /// use either::*;
    ///
    /// let values = [Left(1), Right("the right value")];
    /// assert_eq!(values[0].is_left(), true);
    /// assert_eq!(values[1].is_left(), false);
    /// ```
    pub fn is_left(&self) -> bool {
        match *self {
            Left(_) => true,
            Right(_) => false,
        }
    }

    /// Return true if the value is the `Right` variant.
    ///
    /// ```
    /// use either::*;
    ///
    /// let values = [Left(1), Right("the right value")];
    /// assert_eq!(values[0].is_right(), false);
    /// assert_eq!(values[1].is_right(), true);
    /// ```
    pub fn is_right(&self) -> bool {
        !self.is_left()
    }

    /// Convert the left side of `Either<L, R>` to an `Option<L>`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, ()> = Left("some value");
    /// assert_eq!(left.left(),  Some("some value"));
    ///
    /// let right: Either<(), _> = Right(321);
    /// assert_eq!(right.left(), None);
    /// ```
    pub fn left(self) -> Option<L> {
        match self {
            Left(l) => Some(l),
            Right(_) => None,
        }
    }

    /// Convert the right side of `Either<L, R>` to an `Option<R>`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, ()> = Left("some value");
    /// assert_eq!(left.right(),  None);
    ///
    /// let right: Either<(), _> = Right(321);
    /// assert_eq!(right.right(), Some(321));
    /// ```
    pub fn right(self) -> Option<R> {
        match self {
            Left(_) => None,
            Right(r) => Some(r),
        }
    }

    /// Convert `&Either<L, R>` to `Either<&L, &R>`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, ()> = Left("some value");
    /// assert_eq!(left.as_ref(), Left(&"some value"));
    ///
    /// let right: Either<(), _> = Right("some value");
    /// assert_eq!(right.as_ref(), Right(&"some value"));
    /// ```
    pub fn as_ref(&self) -> Either<&L, &R> {
        match *self {
            Left(ref inner) => Left(inner),
            Right(ref inner) => Right(inner),
        }
    }

    /// Convert `&mut Either<L, R>` to `Either<&mut L, &mut R>`.
    ///
    /// ```
    /// use either::*;
    ///
    /// fn mutate_left(value: &mut Either<u32, u32>) {
    ///     if let Some(l) = value.as_mut().left() {
    ///         *l = 999;
    ///     }
    /// }
    ///
    /// let mut left = Left(123);
    /// let mut right = Right(123);
    /// mutate_left(&mut left);
    /// mutate_left(&mut right);
    /// assert_eq!(left, Left(999));
    /// assert_eq!(right, Right(123));
    /// ```
    pub fn as_mut(&mut self) -> Either<&mut L, &mut R> {
        match *self {
            Left(ref mut inner) => Left(inner),
            Right(ref mut inner) => Right(inner),
        }
    }

    /// Convert `Either<L, R>` to `Either<R, L>`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, ()> = Left(123);
    /// assert_eq!(left.flip(), Right(123));
    ///
    /// let right: Either<(), _> = Right("some value");
    /// assert_eq!(right.flip(), Left("some value"));
    /// ```
    pub fn flip(self) -> Either<R, L> {
        match self {
            Left(l) => Right(l),
            Right(r) => Left(r),
        }
    }

    /// Apply the function `f` on the value in the `Left` variant if it is present rewrapping the
    /// result in `Left`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, u32> = Left(123);
    /// assert_eq!(left.map_left(|x| x * 2), Left(246));
    ///
    /// let right: Either<u32, _> = Right(123);
    /// assert_eq!(right.map_left(|x| x * 2), Right(123));
    /// ```
    pub fn map_left<F, M>(self, f: F) -> Either<M, R>
    where
        F: FnOnce(L) -> M,
    {
        match self {
            Left(l) => Left(f(l)),
            Right(r) => Right(r),
        }
    }

    /// Apply the function `f` on the value in the `Right` variant if it is present rewrapping the
    /// result in `Right`.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, u32> = Left(123);
    /// assert_eq!(left.map_right(|x| x * 2), Left(123));
    ///
    /// let right: Either<u32, _> = Right(123);
    /// assert_eq!(right.map_right(|x| x * 2), Right(246));
    /// ```
    pub fn map_right<F, S>(self, f: F) -> Either<L, S>
    where
        F: FnOnce(R) -> S,
    {
        match self {
            Left(l) => Left(l),
            Right(r) => Right(f(r)),
        }
    }

    /// Apply one of two functions depending on contents, unifying their result. If the value is
    /// `Left(L)` then the first function `f` is applied; if it is `Right(R)` then the second
    /// function `g` is applied.
    ///
    /// ```
    /// use either::*;
    ///
    /// fn square(n: u32) -> i32 { (n * n) as i32 }
    /// fn negate(n: i32) -> i32 { -n }
    ///
    /// let left: Either<u32, i32> = Left(4);
    /// assert_eq!(left.either(square, negate), 16);
    ///
    /// let right: Either<u32, i32> = Right(-4);
    /// assert_eq!(right.either(square, negate), 4);
    /// ```
    pub fn either<F, G, T>(self, f: F, g: G) -> T
    where
        F: FnOnce(L) -> T,
        G: FnOnce(R) -> T,
    {
        match self {
            Left(l) => f(l),
            Right(r) => g(r),
        }
    }

    /// Like `either`, but provide some context to whichever of the
    /// functions ends up being called.
    ///
    /// ```
    /// // In this example, the context is a mutable reference
    /// use either::*;
    ///
    /// let mut result = Vec::new();
    ///
    /// let values = vec![Left(2), Right(2.7)];
    ///
    /// for value in values {
    ///     value.either_with(&mut result,
    ///                       |ctx, integer| ctx.push(integer),
    ///                       |ctx, real| ctx.push(f64::round(real) as i32));
    /// }
    ///
    /// assert_eq!(result, vec![2, 3]);
    /// ```
    pub fn either_with<Ctx, F, G, T>(self, ctx: Ctx, f: F, g: G) -> T
    where
        F: FnOnce(Ctx, L) -> T,
        G: FnOnce(Ctx, R) -> T,
    {
        match self {
            Left(l) => f(ctx, l),
            Right(r) => g(ctx, r),
        }
    }

    /// Apply the function `f` on the value in the `Left` variant if it is present.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, u32> = Left(123);
    /// assert_eq!(left.left_and_then::<_,()>(|x| Right(x * 2)), Right(246));
    ///
    /// let right: Either<u32, _> = Right(123);
    /// assert_eq!(right.left_and_then(|x| Right::<(), _>(x * 2)), Right(123));
    /// ```
    pub fn left_and_then<F, S>(self, f: F) -> Either<S, R>
    where
        F: FnOnce(L) -> Either<S, R>,
    {
        match self {
            Left(l) => f(l),
            Right(r) => Right(r),
        }
    }

    /// Apply the function `f` on the value in the `Right` variant if it is present.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, u32> = Left(123);
    /// assert_eq!(left.right_and_then(|x| Right(x * 2)), Left(123));
    ///
    /// let right: Either<u32, _> = Right(123);
    /// assert_eq!(right.right_and_then(|x| Right(x * 2)), Right(246));
    /// ```
    pub fn right_and_then<F, S>(self, f: F) -> Either<L, S>
    where
        F: FnOnce(R) -> Either<L, S>,
    {
        match self {
            Left(l) => Left(l),
            Right(r) => f(r),
        }
    }

    /// Convert the inner value to an iterator.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, Vec<u32>> = Left(vec![1, 2, 3, 4, 5]);
    /// let mut right: Either<Vec<u32>, _> = Right(vec![]);
    /// right.extend(left.into_iter());
    /// assert_eq!(right, Right(vec![1, 2, 3, 4, 5]));
    /// ```
    pub fn into_iter(self) -> Either<L::IntoIter, R::IntoIter>
    where
        L: IntoIterator,
        R: IntoIterator<Item = L::Item>,
    {
        match self {
            Left(l) => Left(l.into_iter()),
            Right(r) => Right(r.into_iter()),
        }
    }

    /// Return left value or given value
    ///
    /// Arguments passed to `left_or` are eagerly evaluated; if you are passing
    /// the result of a function call, it is recommended to use [`left_or_else`],
    /// which is lazily evaluated.
    ///
    /// [`left_or_else`]: #method.left_or_else
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<&str, &str> = Left("left");
    /// assert_eq!(left.left_or("foo"), "left");
    ///
    /// let right: Either<&str, &str> = Right("right");
    /// assert_eq!(right.left_or("left"), "left");
    /// ```
    pub fn left_or(self, other: L) -> L {
        match self {
            Either::Left(l) => l,
            Either::Right(_) => other,
        }
    }

    /// Return left or a default
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<String, u32> = Left("left".to_string());
    /// assert_eq!(left.left_or_default(), "left");
    ///
    /// let right: Either<String, u32> = Right(42);
    /// assert_eq!(right.left_or_default(), String::default());
    /// ```
    pub fn left_or_default(self) -> L
    where
        L: Default,
    {
        match self {
            Either::Left(l) => l,
            Either::Right(_) => L::default(),
        }
    }

    /// Returns left value or computes it from a closure
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<String, u32> = Left("3".to_string());
    /// assert_eq!(left.left_or_else(|_| unreachable!()), "3");
    ///
    /// let right: Either<String, u32> = Right(3);
    /// assert_eq!(right.left_or_else(|x| x.to_string()), "3");
    /// ```
    pub fn left_or_else<F>(self, f: F) -> L
    where
        F: FnOnce(R) -> L,
    {
        match self {
            Either::Left(l) => l,
            Either::Right(r) => f(r),
        }
    }

    /// Return right value or given value
    ///
    /// Arguments passed to `right_or` are eagerly evaluated; if you are passing
    /// the result of a function call, it is recommended to use [`right_or_else`],
    /// which is lazily evaluated.
    ///
    /// [`right_or_else`]: #method.right_or_else
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let right: Either<&str, &str> = Right("right");
    /// assert_eq!(right.right_or("foo"), "right");
    ///
    /// let left: Either<&str, &str> = Left("left");
    /// assert_eq!(left.right_or("right"), "right");
    /// ```
    pub fn right_or(self, other: R) -> R {
        match self {
            Either::Left(_) => other,
            Either::Right(r) => r,
        }
    }

    /// Return right or a default
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<String, u32> = Left("left".to_string());
    /// assert_eq!(left.right_or_default(), u32::default());
    ///
    /// let right: Either<String, u32> = Right(42);
    /// assert_eq!(right.right_or_default(), 42);
    /// ```
    pub fn right_or_default(self) -> R
    where
        R: Default,
    {
        match self {
            Either::Left(_) => R::default(),
            Either::Right(r) => r,
        }
    }

    /// Returns right value or computes it from a closure
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<String, u32> = Left("3".to_string());
    /// assert_eq!(left.right_or_else(|x| x.parse().unwrap()), 3);
    ///
    /// let right: Either<String, u32> = Right(3);
    /// assert_eq!(right.right_or_else(|_| unreachable!()), 3);
    /// ```
    pub fn right_or_else<F>(self, f: F) -> R
    where
        F: FnOnce(L) -> R,
    {
        match self {
            Either::Left(l) => f(l),
            Either::Right(r) => r,
        }
    }

    /// Returns the left value
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<_, ()> = Left(3);
    /// assert_eq!(left.unwrap_left(), 3);
    /// ```
    ///
    /// # Panics
    ///
    /// When `Either` is a `Right` value
    ///
    /// ```should_panic
    /// # use either::*;
    /// let right: Either<(), _> = Right(3);
    /// right.unwrap_left();
    /// ```
    pub fn unwrap_left(self) -> L
    where
        R: std::fmt::Debug,
    {
        match self {
            Either::Left(l) => l,
            Either::Right(r) => {
                panic!("called `Either::unwrap_left()` on a `Right` value: {:?}", r)
            }
        }
    }

    /// Returns the right value
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let right: Either<(), _> = Right(3);
    /// assert_eq!(right.unwrap_right(), 3);
    /// ```
    ///
    /// # Panics
    ///
    /// When `Either` is a `Left` value
    ///
    /// ```should_panic
    /// # use either::*;
    /// let left: Either<_, ()> = Left(3);
    /// left.unwrap_right();
    /// ```
    pub fn unwrap_right(self) -> R
    where
        L: std::fmt::Debug,
    {
        match self {
            Either::Right(r) => r,
            Either::Left(l) => panic!("called `Either::unwrap_right()` on a `Left` value: {:?}", l),
        }
    }

    /// Returns the left value
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let left: Either<_, ()> = Left(3);
    /// assert_eq!(left.expect_left("value was Right"), 3);
    /// ```
    ///
    /// # Panics
    ///
    /// When `Either` is a `Right` value
    ///
    /// ```should_panic
    /// # use either::*;
    /// let right: Either<(), _> = Right(3);
    /// right.expect_left("value was Right");
    /// ```
    pub fn expect_left(self, msg: &str) -> L
    where
        R: std::fmt::Debug,
    {
        match self {
            Either::Left(l) => l,
            Either::Right(r) => panic!("{}: {:?}", msg, r),
        }
    }

    /// Returns the right value
    ///
    /// # Examples
    ///
    /// ```
    /// # use either::*;
    /// let right: Either<(), _> = Right(3);
    /// assert_eq!(right.expect_right("value was Left"), 3);
    /// ```
    ///
    /// # Panics
    ///
    /// When `Either` is a `Left` value
    ///
    /// ```should_panic
    /// # use either::*;
    /// let left: Either<_, ()> = Left(3);
    /// left.expect_right("value was Right");
    /// ```
    pub fn expect_right(self, msg: &str) -> R
    where
        L: std::fmt::Debug,
    {
        match self {
            Either::Right(r) => r,
            Either::Left(l) => panic!("{}: {:?}", msg, l),
        }
    }
}

impl<T, L, R> Either<(T, L), (T, R)> {
    /// Factor out a homogeneous type from an either of pairs.
    ///
    /// Here, the homogeneous type is the first element of the pairs.
    ///
    /// ```
    /// use either::*;
    /// let left: Either<_, (u32, String)> = Left((123, vec![0]));
    /// assert_eq!(left.factor_first().0, 123);
    ///
    /// let right: Either<(u32, Vec<u8>), _> = Right((123, String::new()));
    /// assert_eq!(right.factor_first().0, 123);
    /// ```
    pub fn factor_first(self) -> (T, Either<L, R>) {
        match self {
            Left((t, l)) => (t, Left(l)),
            Right((t, r)) => (t, Right(r)),
        }
    }
}

impl<T, L, R> Either<(L, T), (R, T)> {
    /// Factor out a homogeneous type from an either of pairs.
    ///
    /// Here, the homogeneous type is the second element of the pairs.
    ///
    /// ```
    /// use either::*;
    /// let left: Either<_, (String, u32)> = Left((vec![0], 123));
    /// assert_eq!(left.factor_second().1, 123);
    ///
    /// let right: Either<(Vec<u8>, u32), _> = Right((String::new(), 123));
    /// assert_eq!(right.factor_second().1, 123);
    /// ```
    pub fn factor_second(self) -> (Either<L, R>, T) {
        match self {
            Left((l, t)) => (Left(l), t),
            Right((r, t)) => (Right(r), t),
        }
    }
}

impl<T> Either<T, T> {
    /// Extract the value of an either over two equivalent types.
    ///
    /// ```
    /// use either::*;
    ///
    /// let left: Either<_, u32> = Left(123);
    /// assert_eq!(left.into_inner(), 123);
    ///
    /// let right: Either<u32, _> = Right(123);
    /// assert_eq!(right.into_inner(), 123);
    /// ```
    pub fn into_inner(self) -> T {
        either!(self, inner => inner)
    }

    /// Map `f` over the contained value and return the result in the
    /// corresponding variant.
    ///
    /// ```
    /// use either::*;
    ///
    /// let value: Either<_, i32> = Right(42);
    ///
    /// let other = value.map(|x| x * 2);
    /// assert_eq!(other, Right(84));
    /// ```
    pub fn map<F, M>(self, f: F) -> Either<M, M>
    where
        F: FnOnce(T) -> M,
    {
        match self {
            Left(l) => Left(f(l)),
            Right(r) => Right(f(r)),
        }
    }
}

/// Convert from `Result` to `Either` with `Ok => Right` and `Err => Left`.
impl<L, R> From<Result<R, L>> for Either<L, R> {
    fn from(r: Result<R, L>) -> Self {
        match r {
            Err(e) => Left(e),
            Ok(o) => Right(o),
        }
    }
}

/// Convert from `Either` to `Result` with `Right => Ok` and `Left => Err`.
impl<L, R> Into<Result<R, L>> for Either<L, R> {
    fn into(self) -> Result<R, L> {
        match self {
            Left(l) => Err(l),
            Right(r) => Ok(r),
        }
    }
}

impl<L, R, A> Extend<A> for Either<L, R>
where
    L: Extend<A>,
    R: Extend<A>,
{
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = A>,
    {
        either!(*self, ref mut inner => inner.extend(iter))
    }
}

/// `Either<L, R>` is an iterator if both `L` and `R` are iterators.
impl<L, R> Iterator for Either<L, R>
where
    L: Iterator,
    R: Iterator<Item = L::Item>,
{
    type Item = L::Item;

    fn next(&mut self) -> Option<Self::Item> {
        either!(*self, ref mut inner => inner.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        either!(*self, ref inner => inner.size_hint())
    }

    fn fold<Acc, G>(self, init: Acc, f: G) -> Acc
    where
        G: FnMut(Acc, Self::Item) -> Acc,
    {
        either!(self, inner => inner.fold(init, f))
    }

    fn count(self) -> usize {
        either!(self, inner => inner.count())
    }

    fn last(self) -> Option<Self::Item> {
        either!(self, inner => inner.last())
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        either!(*self, ref mut inner => inner.nth(n))
    }

    fn collect<B>(self) -> B
    where
        B: iter::FromIterator<Self::Item>,
    {
        either!(self, inner => inner.collect())
    }

    fn all<F>(&mut self, f: F) -> bool
    where
        F: FnMut(Self::Item) -> bool,
    {
        either!(*self, ref mut inner => inner.all(f))
    }
}

impl<L, R> DoubleEndedIterator for Either<L, R>
where
    L: DoubleEndedIterator,
    R: DoubleEndedIterator<Item = L::Item>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        either!(*self, ref mut inner => inner.next_back())
    }
}

impl<L, R> ExactSizeIterator for Either<L, R>
where
    L: ExactSizeIterator,
    R: ExactSizeIterator<Item = L::Item>,
{
}

#[cfg(any(test, feature = "use_std"))]
/// `Either<L, R>` implements `Read` if both `L` and `R` do.
///
/// Requires crate feature `"use_std"`
impl<L, R> Read for Either<L, R>
where
    L: Read,
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        either!(*self, ref mut inner => inner.read(buf))
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        either!(*self, ref mut inner => inner.read_to_end(buf))
    }
}

#[cfg(any(test, feature = "use_std"))]
/// Requires crate feature `"use_std"`
impl<L, R> BufRead for Either<L, R>
where
    L: BufRead,
    R: BufRead,
{
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        either!(*self, ref mut inner => inner.fill_buf())
    }

    fn consume(&mut self, amt: usize) {
        either!(*self, ref mut inner => inner.consume(amt))
    }
}

#[cfg(any(test, feature = "use_std"))]
/// `Either<L, R>` implements `Write` if both `L` and `R` do.
///
/// Requires crate feature `"use_std"`
impl<L, R> Write for Either<L, R>
where
    L: Write,
    R: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        either!(*self, ref mut inner => inner.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        either!(*self, ref mut inner => inner.flush())
    }
}

impl<L, R, Target> AsRef<Target> for Either<L, R>
where
    L: AsRef<Target>,
    R: AsRef<Target>,
{
    fn as_ref(&self) -> &Target {
        either!(*self, ref inner => inner.as_ref())
    }
}

macro_rules! impl_specific_ref_and_mut {
    ($t:ty, $($attr:meta),* ) => {
        $(#[$attr])*
        impl<L, R> AsRef<$t> for Either<L, R>
            where L: AsRef<$t>, R: AsRef<$t>
        {
            fn as_ref(&self) -> &$t {
                either!(*self, ref inner => inner.as_ref())
            }
        }

        $(#[$attr])*
        impl<L, R> AsMut<$t> for Either<L, R>
            where L: AsMut<$t>, R: AsMut<$t>
        {
            fn as_mut(&mut self) -> &mut $t {
                either!(*self, ref mut inner => inner.as_mut())
            }
        }
    };
}

impl_specific_ref_and_mut!(str,);
impl_specific_ref_and_mut!(
    ::std::path::Path,
    cfg(feature = "use_std"),
    doc = "Requires crate feature `use_std`."
);
impl_specific_ref_and_mut!(
    ::std::ffi::OsStr,
    cfg(feature = "use_std"),
    doc = "Requires crate feature `use_std`."
);
impl_specific_ref_and_mut!(
    ::std::ffi::CStr,
    cfg(feature = "use_std"),
    doc = "Requires crate feature `use_std`."
);

impl<L, R, Target> AsRef<[Target]> for Either<L, R>
where
    L: AsRef<[Target]>,
    R: AsRef<[Target]>,
{
    fn as_ref(&self) -> &[Target] {
        either!(*self, ref inner => inner.as_ref())
    }
}

impl<L, R, Target> AsMut<Target> for Either<L, R>
where
    L: AsMut<Target>,
    R: AsMut<Target>,
{
    fn as_mut(&mut self) -> &mut Target {
        either!(*self, ref mut inner => inner.as_mut())
    }
}

impl<L, R, Target> AsMut<[Target]> for Either<L, R>
where
    L: AsMut<[Target]>,
    R: AsMut<[Target]>,
{
    fn as_mut(&mut self) -> &mut [Target] {
        either!(*self, ref mut inner => inner.as_mut())
    }
}

impl<L, R> Deref for Either<L, R>
where
    L: Deref,
    R: Deref<Target = L::Target>,
{
    type Target = L::Target;

    fn deref(&self) -> &Self::Target {
        either!(*self, ref inner => &*inner)
    }
}

impl<L, R> DerefMut for Either<L, R>
where
    L: DerefMut,
    R: DerefMut<Target = L::Target>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        either!(*self, ref mut inner => &mut *inner)
    }
}

#[cfg(any(test, feature = "use_std"))]
/// `Either` implements `Error` if *both* `L` and `R` implement it.
impl<L, R> Error for Either<L, R>
where
    L: Error,
    R: Error,
{
    #[allow(deprecated)]
    fn description(&self) -> &str {
        either!(*self, ref inner => inner.description())
    }

    #[allow(deprecated)]
    #[allow(unknown_lints, bare_trait_objects)]
    fn cause(&self) -> Option<&Error> {
        either!(*self, ref inner => inner.cause())
    }
}

impl<L, R> fmt::Display for Either<L, R>
where
    L: fmt::Display,
    R: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        either!(*self, ref inner => inner.fmt(f))
    }
}

#[test]
fn basic() {
    let mut e = Left(2);
    let r = Right(2);
    assert_eq!(e, Left(2));
    e = r;
    assert_eq!(e, Right(2));
    assert_eq!(e.left(), None);
    assert_eq!(e.right(), Some(2));
    assert_eq!(e.as_ref().right(), Some(&2));
    assert_eq!(e.as_mut().right(), Some(&mut 2));
}

#[test]
fn macros() {
    fn a() -> Either<u32, u32> {
        let x: u32 = try_left!(Right(1337u32));
        Left(x * 2)
    }
    assert_eq!(a(), Right(1337));

    fn b() -> Either<String, &'static str> {
        Right(try_right!(Left("foo bar")))
    }
    assert_eq!(b(), Left(String::from("foo bar")));
}

#[test]
fn deref() {
    fn is_str(_: &str) {}
    let value: Either<String, &str> = Left(String::from("test"));
    is_str(&*value);
}

#[test]
fn iter() {
    let x = 3;
    let mut iter = match x {
        3 => Left(0..10),
        _ => Right(17..),
    };

    assert_eq!(iter.next(), Some(0));
    assert_eq!(iter.count(), 9);
}

#[test]
fn read_write() {
    use std::io;

    let use_stdio = false;
    let mockdata = [0xff; 256];

    let mut reader = if use_stdio {
        Left(io::stdin())
    } else {
        Right(&mockdata[..])
    };

    let mut buf = [0u8; 16];
    assert_eq!(reader.read(&mut buf).unwrap(), buf.len());
    assert_eq!(&buf, &mockdata[..buf.len()]);

    let mut mockbuf = [0u8; 256];
    let mut writer = if use_stdio {
        Left(io::stdout())
    } else {
        Right(&mut mockbuf[..])
    };

    let buf = [1u8; 16];
    assert_eq!(writer.write(&buf).unwrap(), buf.len());
}

#[test]
#[allow(deprecated)]
fn error() {
    let invalid_utf8 = b"\xff";
    let res = if let Err(error) = ::std::str::from_utf8(invalid_utf8) {
        Err(Left(error))
    } else if let Err(error) = "x".parse::<i32>() {
        Err(Right(error))
    } else {
        Ok(())
    };
    assert!(res.is_err());
    res.unwrap_err().description(); // make sure this can be called
}

/// A helper macro to check if AsRef and AsMut are implemented for a given type.
macro_rules! check_t {
    ($t:ty) => {{
        fn check_ref<T: AsRef<$t>>() {}
        fn propagate_ref<T1: AsRef<$t>, T2: AsRef<$t>>() {
            check_ref::<Either<T1, T2>>()
        }
        fn check_mut<T: AsMut<$t>>() {}
        fn propagate_mut<T1: AsMut<$t>, T2: AsMut<$t>>() {
            check_mut::<Either<T1, T2>>()
        }
    }};
}

// This "unused" method is here to ensure that compilation doesn't fail on given types.
fn _unsized_ref_propagation() {
    check_t!(str);

    fn check_array_ref<T: AsRef<[Item]>, Item>() {}
    fn check_array_mut<T: AsMut<[Item]>, Item>() {}

    fn propagate_array_ref<T1: AsRef<[Item]>, T2: AsRef<[Item]>, Item>() {
        check_array_ref::<Either<T1, T2>, _>()
    }

    fn propagate_array_mut<T1: AsMut<[Item]>, T2: AsMut<[Item]>, Item>() {
        check_array_mut::<Either<T1, T2>, _>()
    }
}

// This "unused" method is here to ensure that compilation doesn't fail on given types.
#[cfg(feature = "use_std")]
fn _unsized_std_propagation() {
    check_t!(::std::path::Path);
    check_t!(::std::ffi::OsStr);
    check_t!(::std::ffi::CStr);
}
