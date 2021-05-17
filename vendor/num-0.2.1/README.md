# num

[![crate](https://img.shields.io/crates/v/num.svg)](https://crates.io/crates/num)
[![documentation](https://docs.rs/num/badge.svg)](https://docs.rs/num)
![minimum rustc 1.15](https://img.shields.io/badge/rustc-1.15+-red.svg)
[![Travis status](https://travis-ci.org/rust-num/num.svg?branch=master)](https://travis-ci.org/rust-num/num)

A collection of numeric types and traits for Rust.

This includes new types for big integers, rationals (aka fractions), and complex numbers,
new traits for generic programming on numeric properties like `Integer`,
and generic range iterators.

`num` is a meta-crate, re-exporting items from these sub-crates:

| Repository | Crate | Documentation |
| ---------- | ----- | ------------- |
| [`num-bigint`]   | [![crate][bigint-cb]][bigint-c]     | [![documentation][bigint-db]][bigint-d]
| [`num-complex`]  | [![crate][complex-cb]][complex-c]   | [![documentation][complex-db]][complex-d]
| [`num-integer`]  | [![crate][integer-cb]][integer-c]   | [![documentation][integer-db]][integer-d]
| [`num-iter`]     | [![crate][iter-cb]][iter-c]         | [![documentation][iter-db]][iter-d]
| [`num-rational`] | [![crate][rational-cb]][rational-c] | [![documentation][rational-db]][rational-d]
| [`num-traits`]   | [![crate][traits-cb]][traits-c]     | [![documentation][traits-db]][traits-d]
| ([`num-derive`]) | [![crate][derive-cb]][derive-c]     | [![documentation][derive-db]][derive-d]

Note: `num-derive` is listed here for reference, but it's not directly included
in `num`.  This is a `proc-macro` crate for deriving some of `num`'s traits.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
num = "0.2"
```

and this to your crate root:

```rust
extern crate num;
```

## Features

This crate can be used without the standard library (`#![no_std]`) by disabling
the default `std` feature. Use this in `Cargo.toml`:

```toml
[dependencies.num]
version = "0.2"
default-features = false
```

The `num-bigint` crate is only available when `std` is enabled, and the other
sub-crates may have limited functionality when used without `std`.

Implementations for `i128` and `u128` are only available with Rust 1.26 and
later.  The build script automatically detects this, but you can make it
mandatory by enabling the `i128` crate feature.

The `rand` feature enables randomization traits in `num-bigint` and
`num-complex`.

The `serde` feature enables serialization for types in `num-bigint`,
`num-complex`, and `num-rational`.

The `num` meta-crate no longer supports features to toggle the inclusion of
the individual sub-crates.  If you need such control, you are recommended to
directly depend on your required crates instead.

## Releases

Release notes are available in [RELEASES.md](RELEASES.md).

## Compatibility

The `num` crate as a whole is tested for rustc 1.15 and greater.

The `num-traits`, `num-integer`, and `num-iter` crates are individually tested
for rustc 1.8 and greater, if you require such older compatibility.


[`num-bigint`]: https://github.com/rust-num/num-bigint
[bigint-c]: https://crates.io/crates/num-bigint
[bigint-cb]: https://img.shields.io/crates/v/num-bigint.svg
[bigint-d]: https://docs.rs/num-bigint/
[bigint-db]: https://docs.rs/num-bigint/badge.svg

[`num-complex`]: https://github.com/rust-num/num-complex
[complex-c]: https://crates.io/crates/num-complex
[complex-cb]: https://img.shields.io/crates/v/num-complex.svg
[complex-d]: https://docs.rs/num-complex/
[complex-db]: https://docs.rs/num-complex/badge.svg

[`num-derive`]: https://github.com/rust-num/num-derive
[derive-c]: https://crates.io/crates/num-derive
[derive-cb]: https://img.shields.io/crates/v/num-derive.svg
[derive-d]: https://docs.rs/num-derive/
[derive-db]: https://docs.rs/num-derive/badge.svg

[`num-integer`]: https://github.com/rust-num/num-integer
[integer-c]: https://crates.io/crates/num-integer
[integer-cb]: https://img.shields.io/crates/v/num-integer.svg
[integer-d]: https://docs.rs/num-integer/
[integer-db]: https://docs.rs/num-integer/badge.svg

[`num-iter`]: https://github.com/rust-num/num-iter
[iter-c]: https://crates.io/crates/num-iter
[iter-cb]: https://img.shields.io/crates/v/num-iter.svg
[iter-d]: https://docs.rs/num-iter/
[iter-db]: https://docs.rs/num-iter/badge.svg

[`num-rational`]: https://github.com/rust-num/num-rational
[rational-c]: https://crates.io/crates/num-rational
[rational-cb]: https://img.shields.io/crates/v/num-rational.svg
[rational-d]: https://docs.rs/num-rational/
[rational-db]: https://docs.rs/num-rational/badge.svg

[`num-traits`]: https://github.com/rust-num/num-traits
[traits-c]: https://crates.io/crates/num-traits
[traits-cb]: https://img.shields.io/crates/v/num-traits.svg
[traits-d]: https://docs.rs/num-traits/
[traits-db]: https://docs.rs/num-traits/badge.svg
