# Release 0.2.0

- **breaking change**: There is now a `std` feature, enabled by default, along
  with the implication that building *without* this feature makes this a
  `#[no_std]` crate.
  - The `Float` and `Real` traits are only available when `std` is enabled.
  - Otherwise, the API is unchanged, and num-traits 0.1.43 now re-exports its
    items from num-traits 0.2 for compatibility (the [semver-trick]).

**Contributors**: @cuviper, @termoshtt, @vks

[semver-trick]: https://github.com/dtolnay/semver-trick

# Release 0.1.43

- All items are now re-exported from num-traits 0.2 for compatibility.

# Release 0.1.42

- [num-traits now has its own source repository][num-356] at [rust-num/num-traits][home].
- [`ParseFloatError` now implements `Display`][22].
- [The new `AsPrimitive` trait][17] implements generic casting with the `as` operator.
- [The new `CheckedShl` and `CheckedShr` traits][21] implement generic
  support for the `checked_shl` and `checked_shr` methods on primitive integers.
- [The new `Real` trait][23] offers a subset of `Float` functionality that may be applicable to more
  types, with a blanket implementation for all existing `T: Float` types.

Thanks to @cuviper, @Enet4, @fabianschuiki, @svartalf, and @yoanlcq for their contributions!

[home]: https://github.com/rust-num/num-traits
[num-356]: https://github.com/rust-num/num/pull/356
[17]: https://github.com/rust-num/num-traits/pull/17
[21]: https://github.com/rust-num/num-traits/pull/21
[22]: https://github.com/rust-num/num-traits/pull/22
[23]: https://github.com/rust-num/num-traits/pull/23


# Prior releases

No prior release notes were kept.  Thanks all the same to the many
contributors that have made this crate what it is!
