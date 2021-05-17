memchr
======
The `memchr` crate provides heavily optimized routines for searching bytes.

[![Build status](https://github.com/BurntSushi/rust-memchr/workflows/ci/badge.svg)](https://github.com/BurntSushi/rust-memchr/actions)
[![](http://meritbadge.herokuapp.com/memchr)](https://crates.io/crates/memchr)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).


### Documentation

[https://docs.rs/memchr](https://docs.rs/memchr)


### Overview

The `memchr` function is traditionally provided by libc, but its
performance can vary significantly depending on the specific
implementation of libc that is used. They can range from manually tuned
Assembly implementations (like that found in GNU's libc) all the way to
non-vectorized C implementations (like that found in MUSL).

To smooth out the differences between implementations of libc, at least
on `x86_64` for Rust 1.27+, this crate provides its own implementation of
`memchr` that should perform competitively with the one found in GNU's libc.
The implementation is in pure Rust and has no dependency on a C compiler or an
Assembler.

Additionally, GNU libc also provides an extension, `memrchr`. This crate
provides its own implementation of `memrchr` as well, on top of `memchr2`,
`memchr3`, `memrchr2` and `memrchr3`. The difference between `memchr` and
`memchr2` is that `memchr2` permits finding all occurrences of two bytes
instead of one. Similarly for `memchr3`.

### Compiling without the standard library

memchr links to the standard library by default, but you can disable the
`std` feature if you want to use it in a `#![no_std]` crate:

```toml
[dependencies]
memchr = { version = "2", default-features = false }
```

On x86 platforms, when the `std` feature is disabled, the SSE2
implementation of memchr will be used in compilers that support it. When
`std` is enabled, the AVX implementation of memchr will be used if the CPU
is determined to support it at runtime.

### Using libc

`memchr` is a routine that is part of libc, although this crate does not use
libc by default. Instead, it uses its own routines, which are either vectorized
or generic fallback routines. In general, these should be competitive with
what's in libc, although this has not been tested for all architectures. If
using `memchr` from libc is desirable and a vectorized routine is not otherwise
available in this crate, then enabling the `libc` feature will use libc's
version of `memchr`.

The rest of the functions in this crate, e.g., `memchr2` or `memrchr3`, are not
a standard part of libc, so they will always use the implementations in this
crate. One exception to this is `memrchr`, which is an extension commonly found
on Linux. On Linux, `memrchr` is used in precisely the same scenario as
`memchr`, as described above.


### Minimum Rust version policy

This crate's minimum supported `rustc` version is `1.28.0`.

The current policy is that the minimum Rust version required to use this crate
can be increased in minor version updates. For example, if `crate 1.0` requires
Rust 1.20.0, then `crate 1.0.z` for all values of `z` will also require Rust
1.20.0 or newer. However, `crate 1.y` for `y > 0` may require a newer minimum
version of Rust.

In general, this crate will be conservative with respect to the minimum
supported version of Rust.
