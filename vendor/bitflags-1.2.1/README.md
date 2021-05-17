bitflags
========

[![Build Status](https://travis-ci.com/bitflags/bitflags.svg?branch=master)](https://travis-ci.com/bitflags/bitflags)
[![Join the chat at https://gitter.im/bitflags/Lobby](https://badges.gitter.im/Join%20Chat.svg)](https://gitter.im/bitflags/Lobby?utm_source=badge&utm_medium=badge&utm_content=badge)
[![Latest version](https://img.shields.io/crates/v/bitflags.svg)](https://crates.io/crates/bitflags)
[![Documentation](https://docs.rs/bitflags/badge.svg)](https://docs.rs/bitflags)
![Minimum rustc version](https://img.shields.io/badge/rustc-1.20+-yellow.svg)
![License](https://img.shields.io/crates/l/bitflags.svg)

A Rust macro to generate structures which behave like a set of bitflags

- [Documentation](https://docs.rs/bitflags)
- [Release notes](https://github.com/bitflags/bitflags/releases)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
bitflags = "1.0"
```

and this to your crate root:

```rust
#[macro_use]
extern crate bitflags;
```

## Rust Version Support

The minimum supported Rust version is 1.20 due to use of associated constants.
