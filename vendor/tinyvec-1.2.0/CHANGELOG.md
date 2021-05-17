# Changelog

## 1.2.0

* [Cryptjar](https://github.com/Cryptjar) removed the `A:Array` bound on the struct of `ArrayVec<A:Array>`,
  and added the `from_array_empty` method, which is a `const fn` constructor
  [pr 141](https://github.com/Lokathor/tinyvec/pull/141).

## 1.1.1

* [saethlin](https://github.com/saethlin) contributed many PRs (
  [127](https://github.com/Lokathor/tinyvec/pull/127),
  [128](https://github.com/Lokathor/tinyvec/pull/128),
  [129](https://github.com/Lokathor/tinyvec/pull/129),
  [131](https://github.com/Lokathor/tinyvec/pull/131),
  [132](https://github.com/Lokathor/tinyvec/pull/132)
  ) to help in several benchmarks.

## 1.1.0

* [slightlyoutofphase](https://github.com/slightlyoutofphase)
added "array splat" style syntax to the `array_vec!` and `tiny_vec!` macros.
You can now write `array_vec![true; 5]` and get a length 5 array vec full of `true`,
just like normal array initialization allows. Same goes for `tiny_vec!`.
([pr 118](https://github.com/Lokathor/tinyvec/pull/118))
* [not-a-seagull](https://github.com/not-a-seagull)
added `ArrayVec::into_inner` so that you can get the array out of an `ArrayVec`.
([pr 124](https://github.com/Lokathor/tinyvec/pull/124))

## 1.0.2

* Added license files for the MIT and Apache-2.0 license options.

## 1.0.1

* Display additional features in the [docs.rs/tinyvec](https://docs.rs/tinyvec) documentation.

## 1.0.0

Initial Stable Release.
