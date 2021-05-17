# num-rational

[![crate](https://img.shields.io/crates/v/num-rational.svg)](https://crates.io/crates/num-rational)
[![documentation](https://docs.rs/num-rational/badge.svg)](https://docs.rs/num-rational)
![minimum rustc 1.15](https://img.shields.io/badge/rustc-1.15+-red.svg)
[![Travis status](https://travis-ci.org/rust-num/num-rational.svg?branch=master)](https://travis-ci.org/rust-num/num-rational)

Generic `Rational` numbers (aka fractions) for Rust.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
num-rational = "0.2"
```

and this to your crate root:

```rust
extern crate num_rational;
```

## Features

This crate can be used without the standard library (`#![no_std]`) by disabling
the default `std` feature.  Use this in `Cargo.toml`:

```toml
[dependencies.num-rational]
version = "0.2"
default-features = false
```

Implementations for `i128` and `u128` are only available with Rust 1.26 and
later.  The build script automatically detects this, but you can make it
mandatory by enabling the `i128` crate feature.

## Releases

Release notes are available in [RELEASES.md](RELEASES.md).

## Compatibility

The `num-rational` crate is tested for rustc 1.15 and greater.
