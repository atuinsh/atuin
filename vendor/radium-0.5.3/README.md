# radium

[![Latest Version](https://img.shields.io/crates/v/radium.svg)](https://crates.io/crates/radium)
[![Documentation](https://docs.rs/radium/badge.svg)](https://docs.rs/radium)

`radium` provides a helper trait with a uniform API for interacting with both
atomic types like [`AtomicUsize`], and non-atomic types like [`Cell<usize>`].

This crate is `#![no_std]`-compatible, and uses no non-core types.

For more details, see the trait's documentation.

Additionally, the `radium::types` module provides a set of type aliases for all
of the atomic primitive types (`bool`, signed and unsigned integers up to 64,
`*mut T`) that map to the atomic type when it is present, and the `Cell` wrapper
when it is not. This allows you to have stable, portable, type names that do not
require `cfg` guards to remove them for targets that do not support them.

[`AtomicUsize`]: https://doc.rust-lang.org/core/sync/atomic/struct.AtomicUsize.html
[`Cell<usize>`]: https://doc.rust-lang.org/core/cell/struct.Cell.html

## Target Architecture Compatibility

Not all Rust targets have symbols for atomic types! The compiler knows what
targets have what atomics, but does not yet expose this information on the
stable channel for libraries to use.

As such, `radium` uses a build script to detect the target architecture and emit
our own directives that mark the presence or absence of an atomic integer.

If `radium` does not work for your architecture, please update the build script
to handle your target string and submit a pull request.

---

**@kneecaw** - <https://twitter.com/kneecaw/status/1132695060812849154>
> Feelin' lazy: Has someone already written a helper trait abstracting
> operations over `AtomicUsize` and `Cell<usize>` for generic code which may
> not care about atomicity?

**@ManishEarth** - <https://twitter.com/ManishEarth/status/1132706585300496384>
> no but call the crate radium
>
> (since people didn't care that it was radioactive and used it in everything)
