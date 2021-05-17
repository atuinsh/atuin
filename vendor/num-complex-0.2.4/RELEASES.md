# Release 0.2.4 (2020-01-09)

- [`Complex::new` is now a `const fn` for Rust 1.31 and later][63].
- [Updated the `autocfg` build dependency to 1.0][68].

**Contributors**: @burrbull, @cuviper, @dingelish

[63]: https://github.com/rust-num/num-complex/pull/63
[68]: https://github.com/rust-num/num-complex/pull/68

# Release 0.2.3 (2019-06-11)

- [`Complex::sqrt()` is now more accurate for negative reals][60].
- [`Complex::cbrt()` computes the principal cube root][61].

**Contributors**: @cuviper

[60]: https://github.com/rust-num/num-complex/pull/60
[61]: https://github.com/rust-num/num-complex/pull/61

# Release 0.2.2 (2019-06-10)

- [`Complex::l1_norm()` computes the Manhattan distance from the origin][43].
- [`Complex::fdiv()` and `finv()` use floating-point for inversion][41], which
  may avoid overflows for some inputs, at the cost of trigonometric rounding.
- [`Complex` now implements `num_traits::MulAdd` and `MulAddAssign`][44].
- [`Complex` now implements `Zero::set_zero` and `One::set_one`][57].
- [`Complex` now implements `num_traits::Pow` and adds `powi` and `powu`][56].

**Contributors**: @adamnemecek, @cuviper, @ignatenkobrain, @Schultzer

[41]: https://github.com/rust-num/num-complex/pull/41
[43]: https://github.com/rust-num/num-complex/pull/43
[44]: https://github.com/rust-num/num-complex/pull/44
[56]: https://github.com/rust-num/num-complex/pull/56
[57]: https://github.com/rust-num/num-complex/pull/57

# Release 0.2.1 (2018-10-08)

- [`Complex` now implements `ToPrimitive`, `FromPrimitive`, `AsPrimitive`, and `NumCast`][33].

**Contributors**: @cuviper, @termoshtt

[33]: https://github.com/rust-num/num-complex/pull/33

# Release 0.2.0 (2018-05-24)

### Enhancements

- [`Complex` now implements `num_traits::Inv` and `One::is_one`][17].
- [`Complex` now implements `Sum` and `Product`][11].
- [`Complex` now supports `i128` and `u128` components][27] with Rust 1.26+.
- [`Complex` now optionally supports `rand` 0.5][28], implementing the
  `Standard` distribution and [a generic `ComplexDistribution`][30].
- [`Rem` with a scalar divisor now avoids `norm_sqr` overflow][25].

### Breaking Changes

- [`num-complex` now requires rustc 1.15 or greater][16].
- [There is now a `std` feature][22], enabled by default, along with the
  implication that building *without* this feature makes this a `#![no_std]`
  crate.  A few methods now require `FloatCore`, and the remaining methods
  based on `Float` are only supported with `std`.
- [The `serde` dependency has been updated to 1.0][7], and `rustc-serialize`
  is no longer supported by `num-complex`.

**Contributors**: @clarcharr, @cuviper, @shingtaklam1324, @termoshtt

[7]: https://github.com/rust-num/num-complex/pull/7
[11]: https://github.com/rust-num/num-complex/pull/11
[16]: https://github.com/rust-num/num-complex/pull/16
[17]: https://github.com/rust-num/num-complex/pull/17
[22]: https://github.com/rust-num/num-complex/pull/22
[25]: https://github.com/rust-num/num-complex/pull/25
[27]: https://github.com/rust-num/num-complex/pull/27
[28]: https://github.com/rust-num/num-complex/pull/28
[30]: https://github.com/rust-num/num-complex/pull/30


# Release 0.1.43 (2018-03-08)

- [Fix a usage typo in README.md][20].

**Contributors**: @shingtaklam1324

[20]: https://github.com/rust-num/num-complex/pull/20


# Release 0.1.42 (2018-02-07)

- [num-complex now has its own source repository][num-356] at [rust-num/num-complex][home].

**Contributors**: @cuviper

[home]: https://github.com/rust-num/num-complex
[num-356]: https://github.com/rust-num/num/pull/356


# Prior releases

No prior release notes were kept.  Thanks all the same to the many
contributors that have made this crate what it is!

