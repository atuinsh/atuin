# num-complex

[![crate](https://img.shields.io/crates/v/num-complex.svg)](https://crates.io/crates/num-complex)
[![documentation](https://docs.rs/num-complex/badge.svg)](https://docs.rs/num-complex)
![minimum rustc 1.15](https://img.shields.io/badge/rustc-1.15+-red.svg)
[![Travis status](https://travis-ci.org/rust-num/num-complex.svg?branch=master)](https://travis-ci.org/rust-num/num-complex)

`Complex` numbers for Rust.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
num-complex = "0.2"
```

and this to your crate root:

```rust
extern crate num_complex;
```

## Features

This crate can be used without the standard library (`#![no_std]`) by disabling
the default `std` feature. Use this in `Cargo.toml`:

```toml
[dependencies.num-complex]
version = "0.2"
default-features = false
```

Features based on `Float` types are only available when `std` is enabled. Where
possible, `FloatCore` is used instead.  Formatting complex numbers only supports
format width when `std` is enabled.

Implementations for `i128` and `u128` are only available with Rust 1.26 and
later.  The build script automatically detects this, but you can make it
mandatory by enabling the `i128` crate feature.

## Releases

Release notes are available in [RELEASES.md](RELEASES.md).

## Compatibility

The `num-complex` crate is tested for rustc 1.15 and greater.
