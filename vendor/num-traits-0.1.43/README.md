# num-traits

[![crate](https://img.shields.io/crates/v/num-traits.svg)](https://crates.io/crates/num-traits)
[![documentation](https://docs.rs/num-traits/badge.svg)](https://docs.rs/num-traits)
[![Travis status](https://travis-ci.org/rust-num/num-traits.svg?branch=master)](https://travis-ci.org/rust-num/num-traits)

Numeric traits for generic mathematics in Rust.

This version of the crate only exists to re-export compatible
items from `num-traits` 0.2.  Please consider updating!

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
num-traits = "0.1"
```

and this to your crate root:

```rust
extern crate num_traits;
```

## Releases

Release notes are available in [RELEASES.md](RELEASES.md).

## Compatibility

The `num-traits` crate is tested for rustc 1.8 and greater.
