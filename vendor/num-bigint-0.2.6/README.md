# num-bigint

[![crate](https://img.shields.io/crates/v/num-bigint.svg)](https://crates.io/crates/num-bigint)
[![documentation](https://docs.rs/num-bigint/badge.svg)](https://docs.rs/num-bigint)
![minimum rustc 1.15](https://img.shields.io/badge/rustc-1.15+-red.svg)
[![Travis status](https://travis-ci.org/rust-num/num-bigint.svg?branch=master)](https://travis-ci.org/rust-num/num-bigint)

Big integer types for Rust, `BigInt` and `BigUint`.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
num-bigint = "0.2"
```

and this to your crate root:

```rust
extern crate num_bigint;
```

## Features

The `std` crate feature is mandatory and enabled by default.  If you depend on
`num-bigint` with `default-features = false`, you must manually enable the
`std` feature yourself.  In the future, we hope to support `#![no_std]` with
the `alloc` crate when `std` is not enabled.

Implementations for `i128` and `u128` are only available with Rust 1.26 and
later.  The build script automatically detects this, but you can make it
mandatory by enabling the `i128` crate feature.

### Random Generation

`num-bigint` supports the generation of random big integers when the `rand`
feature is enabled. To enable it include rand as

```toml
rand = "0.5"
num-bigint = { version = "0.2", features = ["rand"] }
```

Note that you must use the version of `rand` that `num-bigint` is compatible
with: `0.5`.

## Releases

Release notes are available in [RELEASES.md](RELEASES.md).

## Compatibility

The `num-bigint` crate is tested for rustc 1.15 and greater.

## Alternatives

While `num-bigint` strives for good performance in pure Rust code, other
crates may offer better performance with different trade-offs.  The following
table offers a brief comparison to a few alternatives.

| Crate            | License        | Min rustc | Implementation |
| :--------------- | :------------- | :-------- | :------------- |
| **`num-bigint`** | MIT/Apache-2.0 | 1.15      | pure rust |
| [`ramp`]         | Apache-2.0     | nightly   | rust and inline assembly |
| [`rug`]          | LGPL-3.0+      | 1.31      | bundles [GMP] via [`gmp-mpfr-sys`] |
| [`rust-gmp`]     | MIT            | stable?   | links to [GMP] |
| [`apint`]        | MIT/Apache-2.0 | 1.26      | pure rust (unfinished) |

[GMP]: https://gmplib.org/
[`gmp-mpfr-sys`]: https://crates.io/crates/gmp-mpfr-sys
[`rug`]: https://crates.io/crates/rug
[`rust-gmp`]: https://crates.io/crates/rust-gmp
[`ramp`]: https://crates.io/crates/ramp
[`apint`]: https://crates.io/crates/apint
