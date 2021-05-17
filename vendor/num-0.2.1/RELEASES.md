# Release 0.2.1 (2019-01-09)

- Updated all sub-crates to their latest versions.

**Contributors**: @cuviper, @ignatenkobrain, @jimbo1qaz

# Release 0.2.0 (2018-06-29)

All items exported from `num-integer`, `num-iter`, and `num-traits` are still
semver-compatible with those exported by `num` 0.1.  If you have these as public
dependencies in your own crates, it is not a breaking change to move to `num`
0.2.  However, this is not true of `num-bigint`, `num-complex`, or
`num-rational`, as those exported items are distinct in this release.

A few common changes are listed below, but most of the development happens in
the individual sub-crates.  Please consult their release notes for more details
about recent changes:
[`num-bigint`](https://github.com/rust-num/num-bigint/blob/master/RELEASES.md),
[`num-complex`](https://github.com/rust-num/num-complex/blob/master/RELEASES.md),
[`num-integer`](https://github.com/rust-num/num-integer/blob/master/RELEASES.md),
[`num-iter`](https://github.com/rust-num/num-iter/blob/master/RELEASES.md),
[`num-rational`](https://github.com/rust-num/num-rational/blob/master/RELEASES.md),
and [`num-traits`](https://github.com/rust-num/num-traits/blob/master/RELEASES.md).

### Enhancements

- Updates to `num-integer`, `num-iter`, and `num-traits` are still compatible
  with `num` 0.1.
- 128-bit integers are supported with Rust 1.26 and later.
- `BigInt`, `BigUint`, `Complex`, and `Ratio` all implement `Sum` and `Product`.

### Breaking Changes

- `num` now requires rustc 1.15 or greater.
- `num-bigint`, `num-complex`, and `num-rational` have all been updated to 0.2.
- It's no longer possible to toggle individual `num-*` sub-crates using cargo
  features.  If you need that control, please use those crates directly.
- There is now a `std` feature, enabled by default, along with the implication
  that building *without* this feature makes this a `#![no_std]` crate.
  `num::bigint` is not available without `std`, and the other sub-crates may
  have limited functionality.
- The `serde` dependency has been updated to 1.0, still disabled by default.
  The `rustc-serialize` crate is no longer supported by `num`.
- The `rand` dependency has been updated to 0.5, now disabled by default.  This
  requires rustc 1.22 or greater for `rand`'s own requirement.

**Contributors**: @CAD97, @cuviper, and the many sub-crate contributors!

# Release 0.1.42 (2018-02-08)

- [All of the num sub-crates now have their own source repositories][num-356].
- Updated num sub-crates to their latest versions.

**Contributors**: @cuviper

[num-356]: https://github.com/rust-num/num/pull/356


# Prior releases

No prior release notes were kept.  Thanks all the same to the many
contributors that have made this crate what it is!
